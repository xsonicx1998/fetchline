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
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
async fn round87_direct_ytdlp_first_attempt_sends_no_cookies() {
    // Round 87 (item 4) — the per-site consent design only
    // sends a browser session when a video NEEDS a login: the
    // first direct attempt is always cookieless, and the
    // capture's cookies ride only a retry after a
    // LoginRequired failure. The fake fails with "Requested
    // format is not available" (NOT a login wall), so no retry
    // happens and the one argv must carry no --cookies at all —
    // even though the capture carried consented cookies. The
    // format's headers still reach yt-dlp on the first attempt.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        !argv.contains("--cookies"),
        "the first attempt must NOT forward the capture's cookies: {argv}"
    );
    assert!(
        !argv.contains("fetchline-ytdlp-cookies-"),
        "no temp cookie file may be written for the first attempt: {argv}"
    );
    assert!(
        argv.contains("User-Agent: Mozilla/5.0 TestAgent"),
        "the format's User-Agent must reach yt-dlp: {argv}"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        !log.contains("retrying once with the capture's cookies"),
        "a non-login failure must not retry with cookies, got: {log}"
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

// -- Round 87.1 — CdnRefused TikTok retry + scoped cookies --
// A fake yt-dlp that refuses the cookieless first attempt with
// a bare 403, then SUCCEEDS on the retry — and, on the retry,
// dumps the `--cookies` file it was given into the argv log, so
// the test can read what the retry actually sent. Same install
// path / lock as the other fakes: one test at a time.
#[cfg(unix)]
struct FakeYtDlp403ThenSuccess {
    dir: std::path::PathBuf,
}

#[cfg(unix)]
impl FakeYtDlp403ThenSuccess {
    fn install(argv_path: &std::path::Path) -> Self {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let script = dir.join("yt-dlp");
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"2026.10.26\"; exit 0; fi\necho \"$@\" >> \"{}\"\nprev=\"\"; after=0; count=0; outtmpl=\"\"; outdir=\"\"; cfile=\"\"\nfor a in \"$@\"; do\n  if [ \"$after\" = 1 ]; then count=$((count + 1)); continue; fi\n  case \"$prev\" in -o) outtmpl=\"$a\" ;; -P) outdir=\"$a\" ;; --cookies) cfile=\"$a\" ;; esac\n  case \"$a\" in --) after=1 ;; esac\n  prev=\"$a\"\ndone\nif [ -n \"$cfile\" ]; then echo \"---cookies-file---\" >> \"{}\"; cat \"$cfile\" >> \"{}\"; name=$(printf '%s' \"$outtmpl\" | sed 's/%(ext)s/mp4/'); if [ -n \"$outdir\" ]; then target=\"$outdir/$name\"; else target=\"$name\"; fi; printf 'fake-merged-video-bytes' > \"$target\"; echo \"[download] 100% of 22.00B at 1.00B/s ETA 00:00\"; exit 0; fi\necho \"[download]  42.1% of 10.00MiB at  1.00MiB/s ETA 00:10\"\nsleep 1\necho \"ERROR: HTTP Error 403: Forbidden\" >&2\nexit 1\n",
            argv_path.display(),
            argv_path.display(),
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
impl Drop for FakeYtDlp403ThenSuccess {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join("yt-dlp"));
    }
}

#[tokio::test]
#[cfg(unix)]
async fn round87_1_tiktok_cdn_refused_retries_with_scoped_cookies() {
    // Round 87.1 (items 2 + 3) — END TO END through the real
    // direct runner: a TikTok quality-pick capture whose
    // cookies carry their own domains (a `.tiktok.com` page
    // cookie AND a `.tiktokcdn.com` CDN cookie) is confirmed,
    // started, refused with a bare 403 on the cookieless first
    // attempt, and must then retry ONCE with the capture's
    // cookies — the retry's written cookies file must keep the
    // CDN cookie on `.tiktokcdn.com`, not rewrite it to the
    // page host. The fake dumps the cookies file it receives,
    // so the assertion reads what the retry actually sent
    // (the temp file is deleted when the attempt returns, so
    // reading it afterwards would prove nothing).
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(store, 4);
    let store = mgr.store();
    let cookies_json = r#"[{"name":"session","value":"abc123","domain":".tiktok.com","path":"/"},{"name":"ttwid","value":"xyz789","domain":".tiktokcdn.com","path":"/"}]"#;
    let pending = store
        .enqueue_pending_link_resolved(
            "https://v16-webapp-prime.us.tiktok.com/video/tos/useast2a/abc.mp4",
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
                filesize: None,
            },
        )
        .unwrap();
    let (id, _) = mgr
        .confirm_pending_link(&pending.id, None, None, None, None, None, false)
        .await
        .unwrap();
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        dl.direct_ytdlp,
        "a TikTok quality pick must go direct from the start"
    );
    let argv_path = dir.join("fake-argv-87-1.txt");
    let _fake = FakeYtDlp403ThenSuccess::install(&argv_path);
    mgr.start_direct_ytdlp(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "the retry with cookies must complete, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let argv = std::fs::read_to_string(&argv_path).unwrap();
    // An invocation line starts with `--ignore-config` (the
    // first argv element); the cookies-file dump the fake
    // appends adds its own lines below the second invocation,
    // so count invocations by that prefix, not by raw lines.
    let invocations: Vec<&str> = argv
        .lines()
        .filter(|l| l.starts_with("--ignore-config"))
        .collect();
    assert_eq!(
        invocations.len(),
        2,
        "exactly two yt-dlp runs: the cookieless first attempt + one cookie retry: {argv}"
    );
    assert!(
        !invocations[0].contains("--cookies"),
        "the first attempt must NOT forward the capture's cookies: {}",
        invocations[0]
    );
    assert!(
        invocations[1].contains("--cookies"),
        "the retry must forward the capture's cookies: {}",
        invocations[1]
    );
    // The retry's cookies file, as the fake received it: the
    // CDN cookie keeps its own domain, the page cookie keeps
    // its own — neither is rewritten to the page host.
    assert!(
        argv.contains(".tiktokcdn.com\tTRUE\t/\tFALSE\t0\tttwid\txyz789"),
        "the CDN cookie must keep its .tiktokcdn.com domain in the retry's cookies file: {argv}"
    );
    assert!(
        argv.contains(".tiktok.com\tTRUE\t/\tFALSE\t0\tsession\tabc123"),
        "the page cookie must keep its .tiktok.com domain: {argv}"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains(
            "first attempt failed with cdn_refused — retrying once with the capture's cookies"
        ),
        "the retry line must name the real kind (cdn_refused), got: {log}"
    );
    let saved = dir.join("video.mp4");
    assert!(
        saved.is_file(),
        "the retry must write the file: {}",
        saved.display()
    );
}

// -- Round 85 — cookie forwarding + the -o template -----------

#[tokio::test]
async fn round85_tiktok_segmented_download_sends_row_cookies_and_referer() {
    // Round 85 — the cURL removal test PROVED a CDN can require
    // Cookie AND Referer: no-Cookie -> 403, no-Referer -> 403. A
    // capture whose resolve was cookieless used to carry
    // `cookies: []` through the extension untouched, so the
    // pending row got no cookies and the engine GET sent no
    // Cookie at all. The mock's header gate stands in for the
    // CDN: it refuses any request missing the exact Cookie /
    // Referer, so a completing download proves both headers
    // reach the segmented GET — for a stream host (the mock)
    // that is NOT the page host, i.e. no same-host scoping
    // drops them.
    // Round 86 — the page here is deliberately NOT TikTok: a
    // TikTok pick now routes to the direct yt-dlp path in
    // `confirm_pending_link` (Claude Q1), so no TikTok row can
    // reach the segmented engine any more. The segmented path
    // still exists for every other host, and this test proves
    // its header forwarding there.
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        require_headers: vec![
            ("Cookie".to_string(), "session=abc; token=def".to_string()),
            (
                "Referer".to_string(),
                "https://www.example.com/".to_string(),
            ),
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
    let cookies_json = r#"[{"name":"session","value":"abc"},{"name":"token","value":"def"}]"#;
    let pending = store
        .enqueue_pending_link_resolved(
            &server.url,
            Some("https://www.example.com/watch/123"),
            Some(cookies_json),
            &fetchline_engine::store::ResolvedLink {
                filename: Some("video.mp4".to_string()),
                headers: Some(
                    r#"{"User-Agent":"Mozilla/5.0 TestAgent","Referer":"https://www.example.com/"}"#
                        .to_string(),
                ),
                source_url: Some("https://www.example.com/watch/123".to_string()),
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
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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

// -- Round 86 ------------------------------------------------------
// Claude Q1 (quality-pick routing), Q2 (direct logging + forced
// format), Q3 (expiry-aware re-resolve + per-child logging),
// item 4 (empty .part deletion), item 6 (cookie checkpoints),
// ChatGPT (pre-children refresh, supervisor lifecycle).

/// A fake yt-dlp that answers `--dump-json` with the canned JSON
/// at `json_path` (and `--version` with a version), and fails any
/// direct download like the failing fake — so a test can prove
/// the re-resolve path without a real yt-dlp. Same install path
/// / lock as the other fakes: one test at a time.
#[cfg(unix)]
struct FakeYtDlpResolve {
    dir: std::path::PathBuf,
}

#[cfg(unix)]
impl FakeYtDlpResolve {
    fn install(json_path: &std::path::Path, argv_path: &std::path::Path) -> Self {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let script = dir.join("yt-dlp");
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"2026.10.26\"; exit 0; fi\necho \"$@\" >> \"{}\"\nfor a in \"$@\"; do if [ \"$a\" = \"--dump-json\" ]; then cat \"{}\"; exit 0; fi; done\necho \"ERROR: [TikTok] 123: Requested format is not available\" >&2\nexit 1\n",
            argv_path.display(),
            json_path.display()
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
impl Drop for FakeYtDlpResolve {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join("yt-dlp"));
    }
}

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
#[cfg(unix)]
async fn round86_tiktok_quality_pick_routes_to_direct_ytdlp() {
    // Round 86 (Claude Q1) — the R85 retest's TikTok
    // quality-pick (854p) went to the segmented engine on the
    // raw CDN URL and 403'd with 0 B, while the same video
    // downloads fine through direct yt-dlp (the manual TikTok
    // finished 43,464,728 B). `confirm_pending_link` used to
    // send EVERY format_id row to `add_resolved_stream_download`;
    // a TikTok pick now goes direct, exactly like the manual
    // page-URL branch — and the direct run completes through
    // the success fake. A DASH pair (audio_url present) keeps
    // the segmented path: the pending row stores no audio
    // format id, so the direct merge selector cannot be built
    // for it.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            "https://v16-webapp-prime.us.tiktok.com/video/tos/useast2a/abc.mp4",
            Some("https://www.tiktok.com/@user/video/123"),
            Some(cookies_json),
            &fetchline_engine::store::ResolvedLink {
                filename: Some("video.mp4".to_string()),
                headers: Some(
                    r#"{"User-Agent":"Mozilla/5.0 TestAgent","Referer":"https://www.tiktok.com/"}"#
                        .to_string(),
                ),
                source_url: Some("https://www.tiktok.com/@user/video/123".to_string()),
                format_id: Some("854".to_string()),
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
    assert!(
        dl.direct_ytdlp,
        "a TikTok quality-pick must go direct to yt-dlp, not the segmented engine"
    );
    assert_eq!(
        dl.source_url.as_deref(),
        Some("https://www.tiktok.com/@user/video/123")
    );
    assert_eq!(dl.format_id.as_deref(), Some("854"));
    assert!(
        store.get_segments(&id).unwrap().is_empty(),
        "a direct row must have no segments"
    );
    let argv_path = dir.join("fake-argv.txt");
    let _fake = FakeYtDlpSuccess::install(&argv_path);
    mgr.start_direct_ytdlp(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "the direct run must complete, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(dl.downloaded_bytes, 23, "the merged fake file is 23 bytes");
    assert!(
        std::path::Path::new(&dl.save_path).exists(),
        "the merged file must land on the save path: {}",
        dl.save_path
    );
}

#[tokio::test]
#[cfg(unix)]
async fn round86_direct_ytdlp_logs_start_and_success_lines() {
    // Round 86 (Claude Q2) — the R85 success left NO trace in
    // startup.log: `run_ytdlp_direct_inner` logged only the
    // cookies-file failure and the failure line. A start line
    // (id, reason, version, format_id, page host, redacted
    // argv) and a success line (exit 0, size, elapsed ms, path)
    // now bracket every direct run.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(store, 4);
    let (id, _) = mgr
        .add_resolved_video_download(
            "https://www.tiktok.com/@user/video/123",
            &resolved_row("https://example.com/video.mp4"),
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
    let argv_path = dir.join("fake-argv.txt");
    let _fake = FakeYtDlpSuccess::install(&argv_path);
    mgr.start_direct_ytdlp(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "the direct run must complete, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("direct yt-dlp download") && log.contains("start: reason=tiktok"),
        "the start line must name the reason, got: {log}"
    );
    assert!(
        log.contains("yt-dlp 2026.10.26"),
        "the start line must name the yt-dlp version, got: {log}"
    );
    assert!(
        log.contains("format_id 18"),
        "the start line must name the format id, got: {log}"
    );
    assert!(
        log.contains("page host www.tiktok.com"),
        "the start line must name the page host, got: {log}"
    );
    assert!(
        log.contains("argv:"),
        "the start line must carry the redacted argv, got: {log}"
    );
    assert!(
        log.contains("success: exit 0"),
        "the success line must record exit 0, got: {log}"
    );
    assert!(
        log.contains("size 23 bytes"),
        "the success line must record the final size, got: {log}"
    );
    assert!(
        log.contains("elapsed") && log.contains(" ms; path "),
        "the success line must record elapsed ms and the path, got: {log}"
    );
    assert!(
        log.contains("video.mp4"),
        "the success line must name the final file, got: {log}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn round86_forced_ytdlp_format_env_var_replaces_selector() {
    // Round 86 (Claude Q2) — SONIC_FORCE_YTDLP_FORMAT=bogus
    // replaces the -f value so the VPS tail check can force a
    // failure without waiting for a real one. The run fails
    // (the failing fake exits 1) and the argv the fake saw
    // carries `-f bogus`, not the row's format id. The env var
    // is restored on drop, so no other test can see it.
    // Round 87.1 — the hook is gated on the `debug-hooks`
    // cargo feature now (was: debug builds only). With the
    // feature the forced format replaces the selector; without
    // it the hook is dead and the row's own selector reaches
    // yt-dlp — that is the SHIPPED build's behaviour, pinned
    // here so the default suite stays green too.
    struct EnvGuard {
        prev: Option<String>,
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("SONIC_FORCE_YTDLP_FORMAT", v),
                None => std::env::remove_var("SONIC_FORCE_YTDLP_FORMAT"),
            }
        }
    }
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _env_guard = EnvGuard {
        prev: std::env::var("SONIC_FORCE_YTDLP_FORMAT").ok(),
    };
    std::env::set_var("SONIC_FORCE_YTDLP_FORMAT", "bogus");
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(store, 4);
    let (id, _) = mgr
        .add_resolved_video_download(
            "https://www.tiktok.com/@user/video/123",
            &resolved_row("https://example.com/video.mp4"),
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
    let argv_path = dir.join("fake-argv.txt");
    let _fake = FakeYtDlp::install(&argv_path);
    mgr.start_direct_ytdlp(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "error", 30).await,
        "the forced-bogus run must fail, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let argv = std::fs::read_to_string(&argv_path).unwrap();
    if cfg!(feature = "debug-hooks") {
        assert!(
            argv.contains("-f bogus"),
            "the forced format must replace the selector in the argv: {argv}"
        );
        assert!(
            !argv.contains("18/b[ext=mp4]/b"),
            "the row's own selector must not reach yt-dlp: {argv}"
        );
        let log = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            log.contains("start: reason=tiktok") && log.contains("bogus"),
            "the start line must log the forced argv, got: {log}"
        );
    } else {
        assert!(
            argv.contains("18/b[ext=mp4]/b"),
            "without the feature the hook is dead: the row's selector must reach yt-dlp: {argv}"
        );
        assert!(
            !argv.contains("-f bogus"),
            "without the feature the forced format must not reach yt-dlp: {argv}"
        );
    }
}

#[tokio::test]
#[cfg(unix)]
async fn round86_expired_url_reresolves_before_first_request() {
    // Round 86 (Claude Q3) — the R85 YouTube 2160p URL had
    // expired ~5 h before the test, so the 403 was the expected
    // answer. A resolved row whose signed URL is past (or within
    // 120 s) re-resolves BEFORE the first request now. The stale
    // server 403s everything; the fresh server serves — so a
    // completing download proves the first request used the
    // fresh URL, never the stale one.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let stale = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let fresh = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let now = now_epoch_secs();
    let stale_url = format!("{}?expire={}", stale.url, now - 3600);
    let fresh_url = format!("{}?expire={}", fresh.url, now + 20000);
    let json = format!(
        r#"{{"title":"test","extractor":"generic","formats":[{{"format_id":"137","url":"{fresh_url}","ext":"mp4","height":2160,"protocol":"https","vcodec":"avc1.640033","acodec":"none","filesize":524288}}]}}"#
    );
    let dir = tmpdir();
    let json_path = dir.join("fresh.json");
    std::fs::write(&json_path, json).unwrap();
    let argv_path = dir.join("fake-argv.txt");
    let _fake = FakeYtDlpResolve::install(&json_path, &argv_path);
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let row = store
        .create_download(
            &stale_url,
            None,
            "video.mp4",
            &dir.join("video.mp4").to_string_lossy(),
            "cat-video",
            Some(512 * 1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    store
        .set_download_ytdlp_meta(
            &row,
            Some("https://www.youtube.com/watch?v=x"),
            Some("137"),
            false,
        )
        .unwrap();
    // The row is created directly in the store (no probe), so it
    // has no segments yet — `start_download` never probes; give
    // it one open-ended segment like the supervisor test does,
    // or the run fails with "This download has no segments"
    // before it ever touches the fresh URL.
    store.create_segments(&row, &[(0, None)]).unwrap();
    mgr.start_download(&row).await.unwrap();
    assert!(
        wait_for_status(&mgr, &row, "complete", 30).await,
        "the re-resolved download must complete, last status: {}; log: {}",
        mgr.get_download(&row).unwrap().unwrap().status,
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );
    let after = mgr.get_download(&row).unwrap().unwrap();
    assert_eq!(
        after.url, fresh_url,
        "the row must carry the fresh URL after the pre-request re-resolve"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("url expired") && log.contains("re-resolving before the first request"),
        "the expiry re-resolve must be logged, got: {log}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn round86_dash_children_created_from_fresh_url_when_stored_url_expired() {
    // Round 86 (ChatGPT timing finding) — `ensure_dash_children`
    // creates the children with `add_download_with_cookies_and_headers`,
    // which PROBES the stored signed URLs, and
    // `reresolve_dash_parent` only refreshes them AFTER the
    // children exist and a later failure occurs. The re-resolve
    // must happen BEFORE the children are created. The stale
    // servers 403 everything; the fresh server serves — so
    // children that exist at all prove their FIRST probe
    // received the fresh URL, never the stale one. The SAME
    // video format id is kept (no silent quality switch), and
    // the parent's stored URL is refreshed too.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let stale_video = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let stale_audio = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let fresh = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let now = now_epoch_secs();
    let stale_video_url = format!("{}?expire={}", stale_video.url, now - 3600);
    let stale_audio_url = format!("{}?expire={}", stale_audio.url, now - 3600);
    let fresh_video_url = format!("{}?expire={}", fresh.url, now + 20000);
    let fresh_audio_url = format!("{}?expire={}", fresh.url, now + 20000);
    let json = format!(
        r#"{{"title":"test","extractor":"generic","formats":[{{"format_id":"137","url":"{fresh_video_url}","ext":"mp4","height":2160,"protocol":"https","vcodec":"avc1.640033","acodec":"none","filesize":524288}},{{"format_id":"140","url":"{fresh_audio_url}","ext":"m4a","protocol":"https","vcodec":"none","acodec":"mp4a.40.2","filesize":32768}}]}}"#
    );
    let dir = tmpdir();
    let json_path = dir.join("fresh.json");
    std::fs::write(&json_path, json).unwrap();
    let argv_path = dir.join("fake-argv.txt");
    let _fake = FakeYtDlpResolve::install(&json_path, &argv_path);
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let parent = store
        .create_download(
            &stale_video_url,
            None,
            "video.mp4",
            &dir.join("video.mp4").to_string_lossy(),
            "cat-video",
            Some(512 * 1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    store
        .set_download_ytdlp_meta(
            &parent,
            Some("https://www.youtube.com/watch?v=x"),
            Some("137"),
            false,
        )
        .unwrap();
    let (video, audio) = mgr
        .ensure_dash_children(&parent, &stale_audio_url)
        .await
        .expect("children must be created from the fresh URLs");
    assert_eq!(
        video.url, fresh_video_url,
        "the video child's first probe must receive the fresh URL, never the stale one"
    );
    assert_eq!(
        audio.url, fresh_audio_url,
        "the audio child's first probe must receive the fresh URL, never the stale one"
    );
    let parent_after = mgr.get_download(&parent).unwrap().unwrap();
    assert_eq!(
        parent_after.url, fresh_video_url,
        "the parent's stored URL is refreshed too"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("re-resolving before creating the children"),
        "the pre-children re-resolve must be logged, got: {log}"
    );
}

#[tokio::test]
async fn round86_dash_supervisor_logs_start_children_and_failure() {
    // Round 86 (ChatGPT P2) — the supervisor used to log only a
    // child's failure, so a supervisor that never started looked
    // identical to one that ran clean. Start, per-child state
    // (parent id, role, itag, host, expire, status, bytes) and
    // the failure line are all logged now. The children 403 on
    // the mock; the parent fails with the child's message.
    //
    // The video child carries re-resolve meta (source_url +
    // format_id), so its 403 triggers a `resolve()` that spawns
    // the shared `yt-dlp` path: this test MUST hold the fake
    // lock even though it installs no fake, or the resolve runs
    // against a concurrent test's installed fake and pollutes
    // that test's argv log (the R85 DASH test failed exactly
    // this way). With the lock held no fake is installed, so
    // the resolve fails cleanly with BinaryMissing.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut settings = store.get_settings().unwrap();
    settings.max_retries = 1;
    mgr.update_settings(&settings).unwrap();
    let parent = store
        .create_download(
            &server.url,
            None,
            "video.mp4",
            &dir.join("video.mp4").to_string_lossy(),
            "cat-video",
            Some(1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    let future_expire = now_epoch_secs() + 20000;
    let video_url = format!("{}?expire={future_expire}", server.url);
    let video = store
        .create_download(
            &video_url,
            None,
            "video.video.mp4",
            &dir.join("video.video.mp4").to_string_lossy(),
            "cat-video",
            Some(512),
            None,
            None,
            Some(true),
        )
        .unwrap();
    let audio = store
        .create_download(
            &server.url,
            None,
            "video.audio.m4a",
            &dir.join("video.audio.m4a").to_string_lossy(),
            "cat-video",
            Some(512),
            None,
            None,
            Some(true),
        )
        .unwrap();
    store.create_segments(&video, &[(0, None)]).unwrap();
    store.create_segments(&audio, &[(0, None)]).unwrap();
    store
        .link_child_to_parent(&video, &parent, "video")
        .unwrap();
    store
        .link_child_to_parent(&audio, &parent, "audio")
        .unwrap();
    store
        .set_download_ytdlp_meta(
            &video,
            Some("https://www.youtube.com/watch?v=x"),
            Some("137"),
            false,
        )
        .unwrap();
    mgr.resume_download(&parent).await.unwrap();
    assert!(wait_for_status(&mgr, &parent, "error", 60).await);
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains(&format!(
            "dash supervisor {parent}: start video={video} audio={audio}"
        )),
        "the supervisor start must be logged, got: {log}"
    );
    assert!(
        log.contains(&format!("dash {parent} child video {video} itag=137")),
        "the video child line must name its itag, got: {log}"
    );
    assert!(
        log.contains(&format!("host=127.0.0.1 expire={future_expire}")),
        "the child lines must carry host + expire, got: {log}"
    );
    assert!(
        log.contains(&format!("dash {parent} child audio {audio}")),
        "the audio child line must be logged, got: {log}"
    );
    assert!(
        log.contains(&format!("dash {parent} failed:")) && log.contains("downloaded=0"),
        "the failure line must carry the children's downloaded bytes, got: {log}"
    );
}

#[tokio::test]
async fn round86_dash_progress_event_labels_video_and_audio_parts() {
    // Round 86 (Fix A) — a YouTube DASH download used to show
    // its parts as one flat unlabeled list: 9 video parts, then
    // the audio parts numbered 1..6 again (and React keys
    // collided across the children). `emit_dash_progress` now
    // tags each segment with its child kind, so the progress
    // event carries labeled groups. The mock serves 512 B per
    // child; the merge may fail on the fake media bytes, so the
    // parent may end complete or error — either terminal event
    // carries the labeled segments.
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let parent = store
        .create_download(
            &server.url,
            None,
            "video.mp4",
            &dir.join("video.mp4").to_string_lossy(),
            "cat-video",
            Some(1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    let video = store
        .create_download(
            &server.url,
            None,
            "video.video.mp4",
            &dir.join("video.video.mp4").to_string_lossy(),
            "cat-video",
            Some(512),
            None,
            None,
            Some(true),
        )
        .unwrap();
    let audio = store
        .create_download(
            &server.url,
            None,
            "video.audio.m4a",
            &dir.join("video.audio.m4a").to_string_lossy(),
            "cat-video",
            Some(512),
            None,
            None,
            Some(true),
        )
        .unwrap();
    store.create_segments(&video, &[(0, None)]).unwrap();
    store.create_segments(&audio, &[(0, None)]).unwrap();
    store
        .link_child_to_parent(&video, &parent, "video")
        .unwrap();
    store
        .link_child_to_parent(&audio, &parent, "audio")
        .unwrap();
    let mut rx = mgr.subscribe();
    mgr.resume_download(&parent).await.unwrap();
    // Drain events until the parent reaches a terminal status;
    // the supervisor's complete/error emit carries the labeled
    // segments even if the download finishes between ticks. The
    // emit follows the status write, so once the parent is
    // terminal allow a 5 s grace period for the event to arrive
    // before giving up on it.
    let mut labeled: Vec<(String, i64)> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut terminal_since: Option<std::time::Instant> = None;
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await {
            Ok(Ok(ev)) => {
                if ev.id == parent {
                    for s in &ev.segments {
                        if let Some(kind) = &s.child_kind {
                            labeled.push((kind.clone(), s.segment_index));
                        }
                    }
                }
            }
            Ok(Err(_)) => {
                // Lagged or closed: keep draining; the terminal
                // status / deadline breaks the loop below.
            }
            Err(_) => {}
        }
        let row = mgr.get_download(&parent).unwrap().unwrap();
        if row.status == "complete" || row.status == "error" {
            if terminal_since.is_none() {
                terminal_since = Some(std::time::Instant::now());
            }
            if !labeled.is_empty()
                || terminal_since.unwrap().elapsed() > std::time::Duration::from_secs(5)
            {
                break;
            }
        }
        if std::time::Instant::now() > deadline {
            break;
        }
    }
    assert!(
        labeled.iter().any(|(k, _)| k == "video"),
        "a progress event must label a video part, got: {labeled:?}"
    );
    assert!(
        labeled.iter().any(|(k, _)| k == "audio"),
        "a progress event must label an audio part, got: {labeled:?}"
    );
    let mut pairs = labeled.clone();
    pairs.sort();
    pairs.dedup();
    assert_eq!(
        pairs.len(),
        labeled.len(),
        "no (child_kind, segment_index) pair may repeat — the old flat list numbered both children from 0, got: {labeled:?}"
    );
}

#[tokio::test]
async fn round86_confirm_logs_cookie_count_not_values() {
    // Round 86 (item 6) — the R85 retest could not tell whether
    // the CDN GET lacked cookies, because no stage logged even
    // their presence. `confirm_pending_link` now logs the cookie
    // COUNT and the host — never values. The mock's header gate
    // also proves the Cookie header itself still reaches the
    // segmented GET (the R85 proof, kept here with a non-TikTok
    // page: a TikTok pick now goes direct and never reaches the
    // segmented engine).
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        require_headers: vec![
            ("Cookie".to_string(), "session=abc; token=def".to_string()),
            (
                "Referer".to_string(),
                "https://www.example.com/".to_string(),
            ),
        ],
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
    let store = mgr.store();
    let cookies_json = r#"[{"name":"session","value":"abc"},{"name":"token","value":"def"}]"#;
    let pending = store
        .enqueue_pending_link_resolved(
            &server.url,
            Some("https://www.example.com/watch/123"),
            Some(cookies_json),
            &fetchline_engine::store::ResolvedLink {
                filename: Some("video.mp4".to_string()),
                headers: Some(
                    r#"{"User-Agent":"Mozilla/5.0 TestAgent","Referer":"https://www.example.com/"}"#
                        .to_string(),
                ),
                source_url: Some("https://www.example.com/watch/123".to_string()),
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
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("cookies present (2)") && log.contains("host 127.0.0.1"),
        "the confirm checkpoint must log the count + host, got: {log}"
    );
    assert!(
        !log.contains("session=abc") && !log.contains("token=def"),
        "the checkpoint must never log cookie values, got: {log}"
    );
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "the segmented download must complete once Cookie + Referer reach the GET, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
}

#[tokio::test]
async fn round86_failed_download_with_no_bytes_deletes_empty_part() {
    // Round 86 (item 4) — the R85 retest found a 43,464,728-byte
    // `.part` that is empty: `run_inner` pre-allocates it to full
    // size, and a Failed download used to leave it behind. The
    // probe's HEAD still succeeds here, so the run starts and
    // pre-allocates; every segment GET then fails, the run errors
    // with 0 real bytes, and the empty `.part` must be gone.
    // Round 87 fix: this test used to create the row with NO
    // segments, so `run_inner` failed with "This download has no
    // segments" BEFORE `set_len` pre-allocation — the assertion
    // passed without exercising the fix. One segment is now
    // created explicitly.
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    server.set_fail_get(true);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let save_path = dir.join("video.mp4").to_string_lossy().to_string();
    let id = mgr
        .store()
        .create_download(
            &server.url,
            None,
            "video.mp4",
            &save_path,
            "cat-video",
            Some(512 * 1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    mgr.store().create_segments(&id, &[(0, None)]).unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "error", 60).await,
        "the download must fail once every GET fails"
    );
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        dl.downloaded_bytes, 0,
        "no real byte may be downloaded: {}",
        dl.downloaded_bytes
    );
    assert!(
        !std::path::Path::new(&format!("{save_path}.part")).exists(),
        "the empty pre-allocated .part must be deleted on Failed"
    );
    assert!(
        !std::path::Path::new(&save_path).exists(),
        "no final file may exist for a failed download"
    );
}

// -- Round 87 ---------------------------------------------------------------

/// Round 87 — restores an env var on drop (file-level; the
/// Round 86 forced-format test has its own function-local one).
struct EnvGuard {
    name: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let prev = std::env::var(name).ok();
        std::env::set_var(name, value);
        Self { name, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.name, v),
            None => std::env::remove_var(self.name),
        }
    }
}

#[cfg(unix)]
struct FakeYtDlpLoginRetry {
    dir: std::path::PathBuf,
}

#[cfg(unix)]
impl FakeYtDlpLoginRetry {
    /// A fake yt-dlp that fails the FIRST invocation with a
    /// login-required error ("Sign in to confirm your age") and
    /// succeeds on the second (writing the merged file, like
    /// `FakeYtDlpSuccess`). The invocation count lives in a
    /// counter file, so the retry is a genuinely separate
    /// process run. `--version` answers without counting.
    fn install(argv_path: &std::path::Path, counter_path: &std::path::Path) -> Self {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let script = dir.join("yt-dlp");
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"2026.10.26\"; exit 0; fi\necho \"$@\" >> \"{}\"\nn=$(cat \"{}\" 2>/dev/null || echo 0)\nn=$((n + 1))\necho \"$n\" > \"{}\"\nif [ \"$n\" = \"1\" ]; then echo \"ERROR: Sign in to confirm your age\" >&2; exit 1; fi\nprev=\"\"; after=0; count=0; outtmpl=\"\"; outdir=\"\"\nfor a in \"$@\"; do\n  if [ \"$after\" = 1 ]; then count=$((count + 1)); continue; fi\n  case \"$prev\" in -o) outtmpl=\"$a\" ;; -P) outdir=\"$a\" ;; esac\n  case \"$a\" in --) after=1 ;; esac\n  prev=\"$a\"\ndone\ncase \"$outtmpl\" in *%*) ;; *) if [ \"$count\" -gt 1 ]; then echo \"ERROR: Fixed output name but more than one file to download: $outtmpl\" >&2; exit 1; fi ;; esac\nname=$(printf '%s' \"$outtmpl\" | sed 's/%(ext)s/mp4/')\nif [ -n \"$outdir\" ]; then target=\"$outdir/$name\"; else target=\"$name\"; fi\nprintf 'fake-merged-video-bytes' > \"$target\"\necho \"[download] 100% of 22.00B at 1.00B/s ETA 00:00\"\nexit 0\n",
            argv_path.display(),
            counter_path.display(),
            counter_path.display(),
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
impl Drop for FakeYtDlpLoginRetry {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join("yt-dlp"));
    }
}

#[tokio::test]
#[cfg(unix)]
async fn round87_direct_ytdlp_retries_with_cookies_after_login_required() {
    // Round 87 (item 4) — the first direct attempt is
    // cookieless; a LoginRequired failure retries ONCE with
    // the capture's (consented) cookies, and the retry
    // completes. The two argv lines prove the order: no
    // --cookies on attempt 1, --cookies on a temp Netscape
    // file on attempt 2 — and the temp file is deleted when
    // the run returns (RAII).
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
    let argv_path = dir.join("fake-argv-retry.txt");
    let counter_path = dir.join("fake-count.txt");
    let _fake = FakeYtDlpLoginRetry::install(&argv_path, &counter_path);
    mgr.start_direct_ytdlp(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "the cookie retry must complete, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(dl.downloaded_bytes, 23, "the merged fake file is 23 bytes");
    let argv = std::fs::read_to_string(&argv_path).unwrap();
    let lines: Vec<&str> = argv.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "exactly two yt-dlp invocations (first attempt + one retry): {argv}"
    );
    assert!(
        !lines[0].contains("--cookies"),
        "attempt 1 must be cookieless: {}",
        lines[0]
    );
    assert!(
        lines[1].contains("--cookies") && lines[1].contains("fetchline-ytdlp-cookies-"),
        "attempt 2 must carry the cookies on a temp Netscape file: {}",
        lines[1]
    );
    let cookie_path = lines[1]
        .split_whitespace()
        .skip_while(|w| *w != "--cookies")
        .nth(1)
        .expect("a --cookies value in the retry argv");
    assert!(
        !std::path::Path::new(cookie_path).exists(),
        "the temp cookie file must be deleted after the run: {cookie_path}"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("first attempt failed with login_required")
            && log.contains("retrying once with the capture's cookies"),
        "the login retry must be logged, got: {log}"
    );
}

#[cfg(unix)]
struct FakeYtDlpProgress {
    dir: std::path::PathBuf,
}

#[cfg(unix)]
impl FakeYtDlpProgress {
    /// A fake yt-dlp that prints advancing `[download]`
    /// progress lines (10% -> 45.2% -> 90% -> 100% of
    /// 10.00MiB, at rising speeds, 1 s apart) and then writes
    /// the merged file and exits 0 — the mock for a real
    /// TikTok direct download's stdout.
    fn install(argv_path: &std::path::Path) -> Self {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let script = dir.join("yt-dlp");
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"2026.10.26\"; exit 0; fi\necho \"$@\" >> \"{}\"\necho \"[download]  10.0% of 10.00MiB at  1.00MiB/s ETA 00:09\"\nsleep 1\necho \"[download]  45.2% of 10.00MiB at  2.50MiB/s ETA 00:05\"\nsleep 1\necho \"[download]  90.0% of 10.00MiB at  3.00MiB/s ETA 00:01\"\nsleep 1\nprev=\"\"; after=0; count=0; outtmpl=\"\"; outdir=\"\"\nfor a in \"$@\"; do\n  if [ \"$after\" = 1 ]; then count=$((count + 1)); continue; fi\n  case \"$prev\" in -o) outtmpl=\"$a\" ;; -P) outdir=\"$a\" ;; esac\n  case \"$a\" in --) after=1 ;; esac\n  prev=\"$a\"\ndone\ncase \"$outtmpl\" in *%*) ;; *) if [ \"$count\" -gt 1 ]; then echo \"ERROR: Fixed output name but more than one file to download: $outtmpl\" >&2; exit 1; fi ;; esac\nname=$(printf '%s' \"$outtmpl\" | sed 's/%(ext)s/mp4/')\nif [ -n \"$outdir\" ]; then target=\"$outdir/$name\"; else target=\"$name\"; fi\nprintf 'fake-merged-video-bytes' > \"$target\"\necho \"[download] 100% of 10.00MiB at  3.00MiB/s ETA 00:00\"\nexit 0\n",
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
impl Drop for FakeYtDlpProgress {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join("yt-dlp"));
    }
}

#[tokio::test]
#[cfg(unix)]
async fn round87_direct_ytdlp_progress_events_advance() {
    // Round 87 (new P1, user-reported 2026-11-21) — a TikTok
    // direct download on his PC showed NO live progress and NO
    // parts: the row looked stalled, then flipped to Finished.
    // Cause: the direct path is a single yt-dlp process, not
    // the segmented engine, so `emit_snapshot` built its events
    // from an empty segment list (0 bytes) with speed 0 — the
    // progress window polls `last_progress`, so it saw a frozen
    // 0% while the file grew. The events must now carry the
    // parsed yt-dlp bytes/speed: collect the downloading events
    // while the fake prints 10% -> 45.2% -> 90% -> 100% and
    // assert they advance (bytes strictly increasing, live
    // speed, the parsed 10 MiB total).
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(store, 4);
    let (id, _) = mgr
        .add_resolved_video_download(
            "https://www.tiktok.com/@user/video/123",
            &resolved_row("https://example.com/video.mp4"),
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
    // Round 87 (refinement A) — the row is created with the
    // resolved size (512 KiB), so the progress window's percent
    // is 0% (an empty bar) from the start, not null (the
    // indeterminate third-full bar) until the first progress line.
    assert_eq!(
        mgr.get_download(&id).unwrap().unwrap().size_bytes,
        Some(512 * 1024),
        "the row is created with the resolved size"
    );
    let argv_path = dir.join("fake-argv-progress.txt");
    let _fake = FakeYtDlpProgress::install(&argv_path);
    let mut rx = mgr.subscribe();
    mgr.start_direct_ytdlp(&id).await.unwrap();
    let mut downloading: Vec<(u64, u64, Option<u64>)> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut complete = false;
    let mut complete_ev: Option<(u64, Option<u64>)> = None;
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(ev)) => {
                if ev.id == id && ev.status == "downloading" {
                    downloading.push((ev.downloaded_bytes, ev.speed_bps, ev.size_bytes));
                }
                if ev.id == id && ev.status == "complete" {
                    complete = true;
                    complete_ev = Some((ev.downloaded_bytes, ev.size_bytes));
                }
            }
            _ => {
                if mgr.get_download(&id).unwrap().unwrap().status == "complete" {
                    complete = true;
                }
            }
        }
        if complete {
            break;
        }
    }
    assert!(
        complete,
        "the direct run must complete, last status: {} error: {:?}",
        mgr.get_download(&id).unwrap().unwrap().status,
        mgr.get_download(&id).unwrap().unwrap().error_message
    );
    let moving: Vec<(u64, u64, Option<u64>)> = downloading
        .iter()
        .filter(|(d, _, _)| *d > 0)
        .copied()
        .collect();
    assert!(
        moving.len() >= 2,
        "at least two live downloading events (bytes > 0), got: {downloading:?}"
    );
    let min_bytes = moving.iter().map(|(d, _, _)| *d).min().unwrap();
    let max_bytes = moving.iter().map(|(d, _, _)| *d).max().unwrap();
    assert!(
        max_bytes > min_bytes,
        "the downloading events must advance ({min_bytes} -> {max_bytes}), got: {downloading:?}"
    );
    assert!(
        moving.iter().any(|(_, s, _)| *s > 0),
        "at least one event must carry a live speed, got: {downloading:?}"
    );
    assert!(
        max_bytes >= 9_000_000,
        "the 90% line must reach the events (>= 9,000,000 bytes), got max {max_bytes} in {downloading:?}"
    );
    assert!(
        moving
            .iter()
            .any(|(_, _, sz)| *sz == Some(10 * 1024 * 1024)),
        "the events must carry the parsed 10 MiB total, got: {downloading:?}"
    );
    // Round 87 (refinement A) — the bar percent derives from
    // downloaded/size, so every downloading event must be
    // consistent with it: downloaded never exceeds the size, and
    // the implied percent (rounded) is one of the fake's printed
    // percents (10 / 45 / 90 / 100 — 45.2% rounds to 45). A stale
    // partial fill while downloaded is 0 B is impossible when the
    // event itself carries the real bytes.
    for (d, _, sz) in &downloading {
        let sz = sz.expect("every downloading event carries a size now");
        assert!(
            *d <= sz,
            "downloaded ({d}) must never exceed the size ({sz}): {downloading:?}"
        );
        if *d > 0 {
            let pct = (*d as f64 / sz as f64 * 100.0).round() as u64;
            assert!(
                [10, 45, 90, 100].contains(&pct),
                "the implied percent ({pct}%) must match a printed progress line: {downloading:?}"
            );
        }
    }
    // Round 87 (steer 3a) — the FINAL state: the complete event
    // must carry downloaded == size (23 == 23 for the fake file),
    // i.e. an implied 100%, and the row must agree (downloaded,
    // size, completed_at) — the Finished window shows
    // "2.9 MB / 2.9 MB" and "100%", not "0 B / 2.9 MB" and "0%".
    // The row turns complete just before the final snapshot is
    // emitted, so wait for the complete event via last_progress
    // when the live channel did not deliver it first.
    let final_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while complete_ev.is_none() && std::time::Instant::now() < final_deadline {
        if let Some(ev) = mgr.last_progress(&id) {
            if ev.status == "complete" {
                complete_ev = Some((ev.downloaded_bytes, ev.size_bytes));
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (final_downloaded, final_size) =
        complete_ev.expect("a complete event must be emitted for the direct download");
    assert_eq!(
        final_downloaded, 23,
        "the complete event must carry the file's bytes, got {final_downloaded}"
    );
    assert_eq!(
        final_size,
        Some(23),
        "the complete event's size must equal the file's bytes, got {final_size:?}"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert_eq!(
        row.downloaded_bytes, 23,
        "the row must hold the file's bytes"
    );
    assert_eq!(
        row.size_bytes,
        Some(23),
        "the row's size must equal the file's bytes"
    );
    assert!(
        row.completed_at.is_some(),
        "completed_at must be set (the completion dialog's fresh check reads it)"
    );
}

#[tokio::test]
async fn round87_facebook_long_arabic_title_downloads_without_os_error_123() {
    // Round 87 — Facebook reel 4444270735823862: the failing title
    // is 313 chars / 540 UTF-8 bytes with three `|` and emoji +
    // Arabic, and the download failed at 0 B with "File error:
    // ... (os error 123)" (ERROR_INVALID_NAME). The pipes were
    // never the failure — every file-creating path already runs
    // through `sanitize_filename`, which maps `|` to `_`. The
    // failure was the LENGTH: an uncapped name becomes a >255-byte
    // path component, refused by the filesystem at the first file
    // creation. A download using the real title must complete, its
    // save name must carry no `|` and must fit in 200 bytes. On
    // Linux without the fix this fails with ENAMETOOLONG (os error
    // 36) — the same class of failure, one component too long.
    const REEL: &str = include_str!("fixtures/ytdlp/facebook_reel_pipes.json");
    let title: String = serde_json::from_str::<serde_json::Value>(REEL).unwrap()["title"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(title.chars().count(), 313, "the fixture is the real title");
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let (id, _) = mgr
        .add_resolved_stream_download(
            &server.url,
            Some(format!("{title}.mp4")),
            None,
            None,
            None,
            None,
            None,
            Some(512 * 1024),
        )
        .await
        .unwrap();
    let row = mgr.get_download(&id).unwrap().unwrap();
    let file_name = std::path::Path::new(&row.save_path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        !file_name.contains('|'),
        "no pipe may reach the save path: {file_name}"
    );
    assert!(
        file_name.len() <= 200,
        "the save name must fit a filesystem component: {} bytes: {file_name}",
        file_name.len()
    );
    assert!(
        file_name.ends_with(".mp4"),
        "the extension is kept: {file_name}"
    );
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "the download must complete, last status: {} error: {:?}",
        mgr.get_download(&id).unwrap().unwrap().status,
        mgr.get_download(&id).unwrap().unwrap().error_message
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.downloaded_bytes, 512 * 1024);
    assert_eq!(
        std::fs::metadata(&row.save_path).unwrap().len(),
        512 * 1024,
        "the file is on disk under the capped name"
    );
}

// -- Round 87: DASH re-resolve (item 5) ------------------------------------

#[cfg(unix)]
struct FakeYtDlpResolveStateful {
    dir: std::path::PathBuf,
}

#[cfg(unix)]
impl FakeYtDlpResolveStateful {
    /// A fake yt-dlp whose FIRST `--dump-json` returns `json_a`
    /// and every later one returns `json_b` (the invocation count
    /// lives in a counter file, like `FakeYtDlpLoginRetry`).
    fn install(
        json_a: &std::path::Path,
        json_b: &std::path::Path,
        argv_path: &std::path::Path,
        count_path: &std::path::Path,
    ) -> Self {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let script = dir.join("yt-dlp");
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"2026.10.26\"; exit 0; fi\necho \"$@\" >> \"{}\"\nfor a in \"$@\"; do if [ \"$a\" = \"--dump-json\" ]; then n=0; [ -f \"{}\" ] && n=$(cat \"{}\"); n=$((n + 1)); echo \"$n\" > \"{}\"; if [ \"$n\" = 1 ]; then cat \"{}\"; else cat \"{}\"; fi; exit 0; fi; done\necho \"ERROR: [generic] test: no format\" >&2\nexit 1\n",
            argv_path.display(),
            count_path.display(),
            count_path.display(),
            count_path.display(),
            json_a.display(),
            json_b.display()
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
impl Drop for FakeYtDlpResolveStateful {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join("yt-dlp"));
    }
}

/// A `--dump-json` body with one video format (137) and one
/// audio format (140) at the given URLs.
fn dash_resolve_json(video_url: &str, audio_url: &str) -> String {
    format!(
        r#"{{"title":"test","extractor":"generic","formats":[{{"format_id":"137","url":"{video_url}","ext":"mp4","height":2160,"protocol":"https","vcodec":"avc1.640033","acodec":"none","filesize":524288}},{{"format_id":"140","url":"{audio_url}","ext":"m4a","protocol":"https","vcodec":"none","acodec":"mp4a.40.2","filesize":32768}}]}}"#
    )
}

#[tokio::test]
#[cfg(unix)]
async fn round87_dash_pair_reresolve_runs_after_prechildren_refresh() {
    // Round 87 (item 5a) — the shared `reresolved` set used to
    // guard the parent and the pair with ONE key, so the
    // pre-children refresh consumed the guard and a later pair
    // re-resolve was skipped. Here the refresh consumes the
    // `pair-refresh` guard (children are created from the A
    // URLs), then BOTH children 403 on their A URLs: the video
    // child re-resolves through its own `row` guard, the audio
    // child (no meta) through the `pair` guard — which must NOT
    // have been consumed by the refresh. The parent ends error
    // (the merge has no ffmpeg next to the test binary), but both
    // children must have been re-resolved to the B URLs first.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_epoch_secs();
    let stale_video = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let stale_audio = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let a_video = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    a_video.set_fail_get_status(403);
    let a_audio = MockServer::start(MockConfig {
        size_bytes: Some(32 * 1024),
        ..Default::default()
    });
    a_audio.set_fail_get_status(403);
    let b_video = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let b_audio = MockServer::start(MockConfig {
        size_bytes: Some(32 * 1024),
        ..Default::default()
    });
    let stale_video_url = format!("{}?expire={}", stale_video.url, now - 3600);
    let stale_audio_url = format!("{}?expire={}", stale_audio.url, now - 3600);
    let a_video_url = format!("{}?expire={}", a_video.url, now + 20000);
    let a_audio_url = format!("{}?expire={}", a_audio.url, now + 20000);
    let b_video_url = format!("{}?expire={}", b_video.url, now + 20000);
    let b_audio_url = format!("{}?expire={}", b_audio.url, now + 20000);
    let dir = tmpdir();
    let json_a_path = dir.join("a.json");
    std::fs::write(&json_a_path, dash_resolve_json(&a_video_url, &a_audio_url)).unwrap();
    let json_b_path = dir.join("b.json");
    std::fs::write(&json_b_path, dash_resolve_json(&b_video_url, &b_audio_url)).unwrap();
    let argv_path = dir.join("fake-argv.txt");
    let count_path = dir.join("fake-count.txt");
    let _fake =
        FakeYtDlpResolveStateful::install(&json_a_path, &json_b_path, &argv_path, &count_path);
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    settings.max_retries = 1;
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let parent = store
        .create_download(
            &stale_video_url,
            None,
            "video.mp4",
            &dir.join("video.mp4").to_string_lossy(),
            "cat-video",
            Some(512 * 1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    store
        .set_download_ytdlp_meta(
            &parent,
            Some("https://www.youtube.com/watch?v=x"),
            Some("137"),
            false,
        )
        .unwrap();
    let (video, audio) = mgr
        .ensure_dash_children(&parent, &stale_audio_url)
        .await
        .expect("children must be created from the refreshed A URLs");
    assert_eq!(
        video.url, a_video_url,
        "the video child must start on the refreshed A URL"
    );
    assert_eq!(
        audio.url, a_audio_url,
        "the audio child must start on the refreshed A URL"
    );
    mgr.resume_download(&parent).await.unwrap();
    assert!(
        wait_for_status(&mgr, &parent, "error", 60).await,
        "the parent must end error (the merge has no ffmpeg here), last status: {}",
        mgr.get_download(&parent).unwrap().unwrap().status
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains(&format!(
            "re-resolved DASH parent {parent}: video 137 + audio 140 refreshed"
        )),
        "the pair re-resolve must run after the pre-children refresh, got: {log}"
    );
    let video_after = mgr.get_download(&video.id).unwrap().unwrap();
    assert_eq!(
        video_after.url, b_video_url,
        "the video child must have been re-resolved to the B URL"
    );
    let audio_after = mgr.get_download(&audio.id).unwrap().unwrap();
    assert_eq!(
        audio_after.url, b_audio_url,
        "the audio child must have been re-resolved to the B URL"
    );
    let argv = std::fs::read_to_string(&argv_path).unwrap();
    assert_eq!(
        argv.lines().filter(|l| l.contains("--dump-json")).count(),
        3,
        "three resolves must run (refresh + video + pair), got argv: {argv}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn round87_dash_prechildren_refresh_failure_fails_loudly() {
    // Round 87 (item 5b) — a failed pair re-resolve must NOT
    // silently fall back to the stale stored URLs: with no fake
    // yt-dlp installed the resolve fails (BinaryMissing, retried
    // once), `ensure_dash_children` must Err, no children may
    // exist, and the stale servers must never have been probed —
    // the children are never created from expired URLs.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_epoch_secs();
    let stale_video = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let stale_audio = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let stale_video_url = format!("{}?expire={}", stale_video.url, now - 3600);
    let stale_audio_url = format!("{}?expire={}", stale_audio.url, now - 3600);
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let parent = store
        .create_download(
            &stale_video_url,
            None,
            "video.mp4",
            &dir.join("video.mp4").to_string_lossy(),
            "cat-video",
            Some(512 * 1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    store
        .set_download_ytdlp_meta(
            &parent,
            Some("https://www.youtube.com/watch?v=x"),
            Some("137"),
            false,
        )
        .unwrap();
    let err = mgr
        .ensure_dash_children(&parent, &stale_audio_url)
        .await
        .expect_err("a failed pre-children refresh must fail loudly");
    assert!(
        err.to_string().contains("expired"),
        "the error must name the expiry, got: {err}"
    );
    assert!(
        mgr.store()
            .list_child_downloads(&parent)
            .unwrap()
            .is_empty(),
        "no children may exist after a failed refresh"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("not creating the children from the expired stored URLs"),
        "the loud failure must be logged, got: {log}"
    );
    assert_eq!(
        stale_video.get_requests(),
        0,
        "the stale video server must never be hit"
    );
    assert_eq!(
        stale_video.head_requests(),
        0,
        "the stale video server must never be probed"
    );
    assert_eq!(
        stale_audio.get_requests(),
        0,
        "the stale audio server must never be hit"
    );
    assert_eq!(
        stale_audio.head_requests(),
        0,
        "the stale audio server must never be probed"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn round87_dash_both_urls_expired_refresh_before_children() {
    // Round 87 (item 5c) — BOTH stored URLs expired: the
    // re-resolve must run BEFORE `ensure_dash_children` creates
    // or probes the children, and refresh BOTH URLs. The stale
    // servers must receive zero requests of any kind — a single
    // HEAD to a stale URL would prove the refresh ran too late.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let stale_video = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let stale_audio = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let fresh = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let now = now_epoch_secs();
    let stale_video_url = format!("{}?expire={}", stale_video.url, now - 3600);
    let stale_audio_url = format!("{}?expire={}", stale_audio.url, now - 3600);
    let fresh_video_url = format!("{}?expire={}", fresh.url, now + 20000);
    let fresh_audio_url = format!("{}?expire={}", fresh.url, now + 20000);
    let json = dash_resolve_json(&fresh_video_url, &fresh_audio_url);
    let dir = tmpdir();
    let json_path = dir.join("fresh.json");
    std::fs::write(&json_path, json).unwrap();
    let argv_path = dir.join("fake-argv.txt");
    let _fake = FakeYtDlpResolve::install(&json_path, &argv_path);
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let parent = store
        .create_download(
            &stale_video_url,
            None,
            "video.mp4",
            &dir.join("video.mp4").to_string_lossy(),
            "cat-video",
            Some(512 * 1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    store
        .set_download_ytdlp_meta(
            &parent,
            Some("https://www.youtube.com/watch?v=x"),
            Some("137"),
            false,
        )
        .unwrap();
    let (video, audio) = mgr
        .ensure_dash_children(&parent, &stale_audio_url)
        .await
        .expect("children must be created from the fresh URLs");
    assert_eq!(
        video.url, fresh_video_url,
        "the video child must carry the fresh URL"
    );
    assert_eq!(
        audio.url, fresh_audio_url,
        "the audio child must carry the fresh URL"
    );
    let parent_after = mgr.get_download(&parent).unwrap().unwrap();
    assert_eq!(
        parent_after.url, fresh_video_url,
        "the parent's stored URL is refreshed too"
    );
    assert_eq!(
        stale_video.get_requests(),
        0,
        "the stale video server must never be hit"
    );
    assert_eq!(
        stale_video.head_requests(),
        0,
        "the stale video server must never be probed — the refresh runs before any probe"
    );
    assert_eq!(
        stale_audio.get_requests(),
        0,
        "the stale audio server must never be hit"
    );
    assert_eq!(
        stale_audio.head_requests(),
        0,
        "the stale audio server must never be probed"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("re-resolving before creating the children"),
        "the pre-children re-resolve must be logged, got: {log}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn round87_expire_shift_forces_prechildren_reresolve() {
    // Round 87 (item 5d) — `SONIC_FORCE_URL_EXPIRE_NOW_PLUS_SECS`
    // shifts "now" in the expire check, so one run can force
    // the pre-children re-resolve branch. Case A (no var):
    // URLs valid for another hour are used as stored — the
    // resolver is never invoked. Case B (shift +7200 s): the
    // same URLs look expired, so the children come from the
    // fresh resolve instead.
    // Round 87.1 — the shift is gated on the `debug-hooks`
    // cargo feature now (was: debug builds only). Without the
    // feature Case B behaves exactly like Case A: the shift is
    // ignored, the stored URLs are used, the resolver never
    // runs — that is the SHIPPED build's behaviour, pinned
    // here so the default suite stays green too.
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = now_epoch_secs();
    // Case A — no shift: the stored URLs are valid.
    {
        let good_video = MockServer::start(MockConfig {
            size_bytes: Some(512 * 1024),
            ..Default::default()
        });
        let good_audio = MockServer::start(MockConfig {
            size_bytes: Some(32 * 1024),
            ..Default::default()
        });
        let fresh = MockServer::start(MockConfig {
            size_bytes: Some(512 * 1024),
            ..Default::default()
        });
        let good_video_url = format!("{}?expire={}", good_video.url, now + 3600);
        let good_audio_url = format!("{}?expire={}", good_audio.url, now + 3600);
        let fresh_video_url = format!("{}?expire={}", fresh.url, now + 20000);
        let fresh_audio_url = format!("{}?expire={}", fresh.url, now + 20000);
        let dir = tmpdir();
        let json_path = dir.join("fresh.json");
        std::fs::write(
            &json_path,
            dash_resolve_json(&fresh_video_url, &fresh_audio_url),
        )
        .unwrap();
        let argv_path = dir.join("fake-argv.txt");
        let _fake = FakeYtDlpResolve::install(&json_path, &argv_path);
        let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
        let mut settings = store.get_settings().unwrap();
        settings.default_download_folder = dir.to_string_lossy().to_string();
        store.update_settings(&settings).unwrap();
        let mgr = DownloadManager::new(Arc::clone(&store), 4);
        let parent = store
            .create_download(
                &good_video_url,
                None,
                "video.mp4",
                &dir.join("video.mp4").to_string_lossy(),
                "cat-video",
                Some(512 * 1024),
                None,
                None,
                Some(true),
            )
            .unwrap();
        store
            .set_download_ytdlp_meta(
                &parent,
                Some("https://www.youtube.com/watch?v=x"),
                Some("137"),
                false,
            )
            .unwrap();
        let (video, audio) = mgr
            .ensure_dash_children(&parent, &good_audio_url)
            .await
            .expect("children must be created from the stored URLs");
        assert_eq!(video.url, good_video_url, "valid URLs are used as stored");
        assert_eq!(audio.url, good_audio_url, "valid URLs are used as stored");
        let argv = std::fs::read_to_string(&argv_path).unwrap_or_default();
        assert!(
            !argv.contains("--dump-json"),
            "the resolver must never be invoked for valid URLs, got argv: {argv}"
        );
    }
    // Case B — the shift makes the same URLs look expired.
    {
        let _shift = EnvGuard::set("SONIC_FORCE_URL_EXPIRE_NOW_PLUS_SECS", "7200");
        let good_video = MockServer::start(MockConfig {
            size_bytes: Some(512 * 1024),
            ..Default::default()
        });
        let good_audio = MockServer::start(MockConfig {
            size_bytes: Some(32 * 1024),
            ..Default::default()
        });
        let fresh = MockServer::start(MockConfig {
            size_bytes: Some(512 * 1024),
            ..Default::default()
        });
        let good_video_url = format!("{}?expire={}", good_video.url, now + 3600);
        let good_audio_url = format!("{}?expire={}", good_audio.url, now + 3600);
        let fresh_video_url = format!("{}?expire={}", fresh.url, now + 20000);
        let fresh_audio_url = format!("{}?expire={}", fresh.url, now + 20000);
        let dir = tmpdir();
        let json_path = dir.join("fresh.json");
        std::fs::write(
            &json_path,
            dash_resolve_json(&fresh_video_url, &fresh_audio_url),
        )
        .unwrap();
        let argv_path = dir.join("fake-argv.txt");
        let _fake = FakeYtDlpResolve::install(&json_path, &argv_path);
        let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
        let log_path = dir.join("startup.log");
        store.set_log_path(log_path.clone());
        let mut settings = store.get_settings().unwrap();
        settings.default_download_folder = dir.to_string_lossy().to_string();
        store.update_settings(&settings).unwrap();
        let mgr = DownloadManager::new(Arc::clone(&store), 4);
        let parent = store
            .create_download(
                &good_video_url,
                None,
                "video.mp4",
                &dir.join("video.mp4").to_string_lossy(),
                "cat-video",
                Some(512 * 1024),
                None,
                None,
                Some(true),
            )
            .unwrap();
        store
            .set_download_ytdlp_meta(
                &parent,
                Some("https://www.youtube.com/watch?v=x"),
                Some("137"),
                false,
            )
            .unwrap();
        let (video, audio) = mgr
            .ensure_dash_children(&parent, &good_audio_url)
            .await
            .expect("children must be created");
        let log = std::fs::read_to_string(&log_path).unwrap();
        if cfg!(feature = "debug-hooks") {
            assert_eq!(
                video.url, fresh_video_url,
                "the shift must force the refresh for the video child"
            );
            assert_eq!(
                audio.url, fresh_audio_url,
                "the shift must force the refresh for the audio child"
            );
            assert!(
                log.contains("re-resolving before creating the children"),
                "the forced refresh must be logged, got: {log}"
            );
        } else {
            assert_eq!(
                video.url, good_video_url,
                "without the feature the shift is ignored: the stored URL is used"
            );
            assert_eq!(
                audio.url, good_audio_url,
                "without the feature the shift is ignored: the stored URL is used"
            );
            assert!(
                !log.contains("re-resolving before creating the children"),
                "without the feature no re-resolve must run, got: {log}"
            );
            let argv = std::fs::read_to_string(&argv_path).unwrap_or_default();
            assert!(
                !argv.contains("--dump-json"),
                "without the feature the resolver must never be invoked, got argv: {argv}"
            );
        }
    }
}

/// Create a DASH parent with linked video/audio children on the
/// given servers (one open-ended segment each), like the R86
/// supervisor test. Returns (parent, video, audio).
fn dash_pair_on_servers(
    store: &Store,
    dir: &std::path::Path,
    video_url: &str,
    audio_url: &str,
    video_size: i64,
    audio_size: i64,
) -> (String, String, String) {
    let parent = store
        .create_download(
            video_url,
            None,
            "video.mp4",
            &dir.join("video.mp4").to_string_lossy(),
            "cat-video",
            Some(video_size + audio_size),
            None,
            None,
            Some(true),
        )
        .unwrap();
    let video = store
        .create_download(
            video_url,
            None,
            "video.video.mp4",
            &dir.join("video.video.mp4").to_string_lossy(),
            "cat-video",
            Some(video_size),
            None,
            None,
            Some(true),
        )
        .unwrap();
    let audio = store
        .create_download(
            audio_url,
            None,
            "video.audio.m4a",
            &dir.join("video.audio.m4a").to_string_lossy(),
            "cat-video",
            Some(audio_size),
            None,
            None,
            Some(true),
        )
        .unwrap();
    store.create_segments(&video, &[(0, None)]).unwrap();
    store.create_segments(&audio, &[(0, None)]).unwrap();
    store
        .link_child_to_parent(&video, &parent, "video")
        .unwrap();
    store
        .link_child_to_parent(&audio, &parent, "audio")
        .unwrap();
    store
        .set_download_ytdlp_meta(
            &video,
            Some("https://www.youtube.com/watch?v=x"),
            Some("137"),
            false,
        )
        .unwrap();
    (parent, video, audio)
}

#[tokio::test]
async fn round87_dash_progress_carries_live_connection_count() {
    // Round 87 (item 7) — `emit_dash_progress` rebuilt its segment
    // states fresh (running:false), so `emit` counted 0 running
    // flags and the progress window showed "0 connections" while
    // the download ran at full speed. The parent event must now
    // carry the children's summed DB active_connections: 2 while
    // both children run, 0 when done. The plain-download control
    // proves the plain path is unchanged (live > 0, complete 0).
    let slow_video = MockServer::start(MockConfig {
        chunk: 1024,
        delay: std::time::Duration::from_millis(50),
        size_bytes: Some(128 * 1024),
        ..Default::default()
    });
    let slow_audio = MockServer::start(MockConfig {
        chunk: 1024,
        delay: std::time::Duration::from_millis(50),
        size_bytes: Some(64 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (parent, _, _) = dash_pair_on_servers(
        &store,
        &dir,
        &slow_video.url,
        &slow_audio.url,
        128 * 1024,
        64 * 1024,
    );
    let mut rx = mgr.subscribe();
    mgr.resume_download(&parent).await.unwrap();
    let mut downloading_active: Vec<usize> = Vec::new();
    let mut terminal_active: Option<usize> = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(ev)) => {
                if ev.id != parent {
                    continue;
                }
                if ev.status == "downloading" {
                    downloading_active.push(ev.active_connections);
                } else if ev.status == "error" || ev.status == "complete" {
                    terminal_active = Some(ev.active_connections);
                    break;
                }
            }
            _ => {
                if matches!(
                    mgr.get_download(&parent).unwrap().unwrap().status.as_str(),
                    "error" | "complete"
                ) {
                    break;
                }
            }
        }
    }
    assert!(
        downloading_active.iter().any(|a| *a == 2),
        "some downloading parent event must carry 2 live connections, got: {downloading_active:?}"
    );
    assert_eq!(
        terminal_active,
        Some(0),
        "the terminal parent event must carry 0 connections, got: {terminal_active:?}"
    );
    // Plain control: one segment, live > 0 while downloading,
    // 0 when complete.
    let slow = MockServer::start(MockConfig {
        chunk: 1024,
        delay: std::time::Duration::from_millis(50),
        size_bytes: Some(128 * 1024),
        ..Default::default()
    });
    let dir2 = tmpdir();
    let mgr2 = test_mgr(&dir2);
    let save_path = dir2.join("plain.mp4").to_string_lossy().to_string();
    let id = mgr2
        .store()
        .create_download(
            &slow.url,
            None,
            "plain.mp4",
            &save_path,
            "cat-video",
            Some(128 * 1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    mgr2.store().create_segments(&id, &[(0, None)]).unwrap();
    let mut rx2 = mgr2.subscribe();
    mgr2.start_download(&id).await.unwrap();
    let mut plain_downloading: Vec<usize> = Vec::new();
    let mut plain_terminal: Option<usize> = None;
    let deadline2 = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline2 {
        match tokio::time::timeout(std::time::Duration::from_millis(200), rx2.recv()).await {
            Ok(Ok(ev)) => {
                if ev.id != id {
                    continue;
                }
                if ev.status == "downloading" {
                    plain_downloading.push(ev.active_connections);
                } else if ev.status == "complete" {
                    plain_terminal = Some(ev.active_connections);
                    break;
                }
            }
            _ => {
                if mgr2.get_download(&id).unwrap().unwrap().status == "complete" {
                    break;
                }
            }
        }
    }
    assert!(
        plain_downloading.iter().any(|a| *a > 0),
        "plain downloading events must carry live connections, got: {plain_downloading:?}"
    );
    assert_eq!(
        plain_terminal,
        Some(0),
        "the plain complete event must carry 0 connections, got: {plain_terminal:?}"
    );
}

#[tokio::test]
async fn round87_dash_children_share_parent_speed_cap() {
    // Round 87 (item 8) — the DASH children used to get a fresh
    // unlimited limiter each, so a 1000 KB/s cap on the parent ran
    // uncapped (16.7 MB/s in the R86 retest). With the parent
    // capped at 100 KB/s, two 512 KiB children on fast servers
    // must share ONE bucket: after 3 s the parent is still
    // downloading with well under 500 KiB (uncapped, 1 MiB would
    // finish in under a second), and it finishes later (the merge
    // fails without ffmpeg, so the parent ends error).
    let fast_video = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let fast_audio = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (parent, _, _) = dash_pair_on_servers(
        &store,
        &dir,
        &fast_video.url,
        &fast_audio.url,
        512 * 1024,
        512 * 1024,
    );
    mgr.set_download_speed_limit(&parent, Some(100)).unwrap();
    mgr.resume_download(&parent).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let mid = mgr.get_download(&parent).unwrap().unwrap();
    assert_eq!(
        mid.status, "downloading",
        "the capped pair must still be downloading after 3 s"
    );
    assert!(
        mid.downloaded_bytes > 50_000 && mid.downloaded_bytes < 500_000,
        "the shared 100 KB/s cap must hold (~300 KiB after 3 s), got {} bytes",
        mid.downloaded_bytes
    );
    assert!(
        wait_for_status(&mgr, &parent, "error", 60).await,
        "the parent must end error (the merge has no ffmpeg here)"
    );
}

#[tokio::test]
async fn round87_failed_segmented_download_with_no_bytes_deletes_empty_part() {
    // Round 87 (item 12) — the R86 proof covered only the direct
    // path (a download failing at format resolution). The
    // segmented path pre-allocates the same empty `.part` via
    // `set_len`, and `finish_run` must delete it on failure. The
    // segment is created explicitly here: without it the run
    // would fail with "This download has no segments" before
    // pre-allocation (the R86 test's vacuity, fixed alongside).
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    server.set_fail_get(true);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let save_path = dir.join("video.mp4").to_string_lossy().to_string();
    let id = mgr
        .store()
        .create_download(
            &server.url,
            None,
            "video.mp4",
            &save_path,
            "cat-video",
            Some(512 * 1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    mgr.store().create_segments(&id, &[(0, None)]).unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "error", 60).await,
        "the segmented download must fail once every GET fails"
    );
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        !dl.error_message
            .as_deref()
            .unwrap_or("")
            .contains("no segments"),
        "the run must really have started (and pre-allocated) — error was: {:?}",
        dl.error_message
    );
    assert_eq!(dl.downloaded_bytes, 0, "no real byte may be downloaded");
    assert!(
        !std::path::Path::new(&format!("{save_path}.part")).exists(),
        "the empty pre-allocated .part must be deleted on Failed"
    );
    assert!(
        !std::path::Path::new(&save_path).exists(),
        "no final file may exist for a failed download"
    );
}

#[tokio::test]
async fn round87_facebook_long_arabic_title_dash_children_complete() {
    // Round 87 — DASH variant of the Facebook reel 4444270735823862
    // title test: the parent is created from the same 313-char
    // title, then `ensure_dash_children` derives the video/audio
    // part filenames from the parent's (capped) stem. Both children
    // must complete — their part files are created from the capped
    // stem, never from the raw title — and the parent must end
    // error only because the merge has no ffmpeg here, never with
    // os error 123 / ENAMETOOLONG.
    const REEL: &str = include_str!("fixtures/ytdlp/facebook_reel_pipes.json");
    let title: String = serde_json::from_str::<serde_json::Value>(REEL).unwrap()["title"]
        .as_str()
        .unwrap()
        .to_string();
    let video_server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let audio_server = MockServer::start(MockConfig {
        size_bytes: Some(32 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let (parent, _) = mgr
        .add_resolved_stream_download(
            &video_server.url,
            Some(format!("{title}.mp4")),
            None,
            None,
            None,
            None,
            None,
            Some(512 * 1024),
        )
        .await
        .unwrap();
    let parent_row = mgr.get_download(&parent).unwrap().unwrap();
    let parent_name = std::path::Path::new(&parent_row.save_path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        !parent_name.contains('|'),
        "no pipe may reach the save path"
    );
    assert!(
        parent_name.len() <= 200,
        "the save name must fit: {parent_name}"
    );
    let (video, audio) = mgr
        .ensure_dash_children(&parent, &audio_server.url)
        .await
        .expect("the DASH children must be created from the capped stem");
    for child in [&video, &audio] {
        let name = std::path::Path::new(&child.save_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            !name.contains('|'),
            "no pipe may reach a child name: {name}"
        );
        assert!(name.len() <= 220, "a child name must fit: {name}");
    }
    mgr.resume_download(&parent).await.unwrap();
    assert!(
        wait_for_status(&mgr, &parent, "error", 60).await,
        "the parent must end error (the merge has no ffmpeg here)"
    );
    let video_after = mgr.get_download(&video.id).unwrap().unwrap();
    let audio_after = mgr.get_download(&audio.id).unwrap().unwrap();
    assert_eq!(
        video_after.status, "complete",
        "the video child must complete — its part file was creatable"
    );
    assert_eq!(
        video_after.downloaded_bytes,
        512 * 1024,
        "the video child must have all its bytes"
    );
    assert_eq!(
        audio_after.status, "complete",
        "the audio child must complete — its part file was creatable"
    );
    assert_eq!(
        audio_after.downloaded_bytes,
        32 * 1024,
        "the audio child must have all its bytes"
    );
    let parent_after = mgr.get_download(&parent).unwrap().unwrap();
    let err = parent_after.error_message.unwrap_or_default();
    assert!(
        !err.contains("os error 123") && !err.contains("os error 36"),
        "the parent must not fail with a filename error, got: {err}"
    );
}

// -- Round 87: live TikTok (item 4) ----------------------------------------

fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
struct RealYtDlp {
    dir: std::path::PathBuf,
    installed: Vec<String>,
}

#[cfg(unix)]
impl RealYtDlp {
    /// Copy the REAL yt-dlp (plus ffmpeg/ffprobe when found on
    /// PATH) next to the test binary, replacing any fake.
    /// Panics with a clear message when yt-dlp is not on PATH —
    /// a live test that silently skipped would prove nothing.
    fn install() -> Self {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let mut installed = Vec::new();
        for name in ["yt-dlp", "ffmpeg", "ffprobe"] {
            if let Some(src) = find_on_path(name) {
                let dest = dir.join(name);
                std::fs::copy(&src, &dest).unwrap();
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
                installed.push(name.to_string());
            } else if name == "yt-dlp" {
                panic!(
                    "FETCHLINE_REAL_TIKTOK_TEST=1 is set but no yt-dlp binary was found on PATH — install yt-dlp to run this live test"
                );
            }
        }
        Self { dir, installed }
    }
}

#[cfg(unix)]
impl Drop for RealYtDlp {
    fn drop(&mut self) {
        for name in &self.installed {
            let _ = std::fs::remove_file(self.dir.join(name));
        }
    }
}

#[tokio::test]
#[cfg(unix)]
async fn round87_tiktok_standard_video_completes_without_cookies_live() {
    // Round 87 (item 4) — the live proof that the standard TikTok
    // video (7223205454032178475, 43,464,728 B) still completes
    // with NO cookies on the first attempt (it did by hand in
    // R84): the run's start line must log an argv with no
    // `--cookies`, no cookie retry may happen, and the success
    // line must appear. Env-gated like the other real-internet
    // tests (`FETCHLINE_REAL_TIKTOK_TEST=1`); it needs real yt-dlp
    // on PATH and internet access, so it cannot run on a bare VM.
    if std::env::var("FETCHLINE_REAL_TIKTOK_TEST").as_deref() != Ok("1") {
        println!("SKIPPED: set FETCHLINE_REAL_TIKTOK_TEST=1 to run the live TikTok download (needs real yt-dlp on PATH + internet)");
        return;
    }
    let _fake_guard = FAKE_YTDLP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _real = RealYtDlp::install();
    let page_url = "https://www.tiktok.com/@chisbetyu81/video/7223205454032178475";
    let exe_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let paths = fetchline_engine::ytdlp::locate_sidecars(&exe_dir);
    let resolved = fetchline_engine::ytdlp::resolve(
        page_url,
        &fetchline_engine::ytdlp::ResolveOptions {
            paths,
            cookies: vec![],
            extractor_args: None,
            timeout_secs: fetchline_engine::ytdlp::RESOLVE_TIMEOUT_SECS,
        },
    )
    .await
    .expect("the TikTok page must resolve cookieless");
    let progressive: Vec<_> = resolved
        .formats
        .iter()
        .filter(|f| f.vcodec.as_deref() != Some("none") && f.acodec.as_deref() != Some("none"))
        .collect();
    let fmt = progressive
        .iter()
        .find(|f| f.format_id == "854")
        .or_else(|| progressive.iter().max_by_key(|f| f.height.unwrap_or(0)))
        .expect("the TikTok resolve must offer a progressive video format");
    let row = fetchline_engine::ytdlp::QualityRow {
        height: fmt.height.unwrap_or(0),
        label: format!("{}p", fmt.height.unwrap_or(0)),
        ext: fmt.ext.clone(),
        has_audio: true,
        needs_merge: false,
        filesize: fmt.filesize,
        url: fmt.url.clone(),
        audio_url: None,
        audio_format_id: None,
        format_id: fmt.format_id.clone(),
        headers: fmt.http_headers.clone(),
        fragmented: fmt.has_fragments,
    };
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_resolved_video_download(
            page_url,
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
    mgr.start_direct_ytdlp(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 300).await,
        "the TikTok video must complete cookieless, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        dl.downloaded_bytes > 1_000_000,
        "the file must be a real video, got {} bytes",
        dl.downloaded_bytes
    );
    assert!(
        dir.join("video.mp4").exists(),
        "the file must exist on disk"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("direct yt-dlp download"),
        "the start line must be logged, got: {log}"
    );
    assert!(
        !log.contains("--cookies"),
        "the first attempt must send no cookies, got: {log}"
    );
    assert!(
        !log.contains("retrying once with the capture's cookies"),
        "no cookie retry may happen for a video that completes cookieless, got: {log}"
    );
    assert!(
        log.contains("success: exit 0"),
        "the success line must be logged, got: {log}"
    );
}
