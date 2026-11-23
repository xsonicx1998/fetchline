//! Round 78 — yt-dlp integration tests: the store migration
//! (new columns on an old database), the host's resolved-video
//! hand-off (a `video_found` with resolver metadata stores it on
//! the pending row), header parsing for the per-download header
//! map, and the DASH merge with the system ffmpeg. No public
//! internet is used: resolves are never run here (no yt-dlp
//! binary, no network) — the resolver itself is unit-tested in
//! `src/ytdlp.rs` against golden `-J` fixtures. Round 86 adds
//! one exception: a fake yt-dlp (installed in the test binary's
//! own dir, like video_tests.rs) drives a direct download to
//! exit 1, so the failure line's flattened `tail:` can be
//! asserted — still no network, still no real binary.

#[path = "common/mock.rs"]
mod mock;

use std::path::Path;
use std::sync::Arc;

use mock::*;
use rusqlite::Connection;

use fetchline_engine::manager::merge_dash_files;
use fetchline_engine::native_host::{handle_host_message, HostMessage};
use fetchline_engine::store::ResolvedLink;
use fetchline_engine::video::VideoVariantInput;
use fetchline_engine::{DownloadManager, Store};

// -- store migration ------------------------------------------------------

#[test]
fn store_migration_adds_round78_columns_to_old_db() {
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        // A database from before Round 78: the real schema with
        // just the Round 78 columns stripped out of the three
        // tables, so `Store::open` must add them back with their
        // defaults. (A hand-written minimal table cannot stand in
        // for the old shape — the schema's own indexes reference
        // the older columns, so the old table has to be complete
        // except for the new ones.)
        let schema = include_str!("../../../replica/schema.sql");
        let old = schema
            .replace(
                "  parent_id TEXT REFERENCES downloads(id) ON DELETE CASCADE,\n",
                "",
            )
            .replace("  child_kind TEXT,\n", "")
            .replace("  merge_status TEXT,\n", "")
            .replace("  source_url TEXT,\n", "")
            .replace("  format_id TEXT,\n", "")
            .replace("  direct_ytdlp INTEGER NOT NULL DEFAULT 0,\n", "")
            .replace("  filename TEXT,\n", "")
            .replace("  headers TEXT,\n", "")
            .replace("  audio_url TEXT,\n", "")
            .replace("  fragmented INTEGER NOT NULL DEFAULT 0,\n", "")
            .replace(
                "  platform_downloads_enabled INTEGER NOT NULL DEFAULT 1,\n",
                "",
            )
            .replace("  ytdlp_extractor_args TEXT,\n", "");
        // `source_url` / `format_id` appear in both tables, so the
        // replaces above stripped them from pending_links too —
        // and the downloads `source_url` line is gone as well.
        // (The Round 78 comment blocks still mention the names;
        // only the column definitions are gone.)
        assert!(!old.contains("  parent_id TEXT"));
        assert!(!old.contains("  direct_ytdlp INTEGER"));
        assert!(!old.contains("  fragmented INTEGER"));
        assert!(!old.contains("  platform_downloads_enabled INTEGER"));
        assert!(!old.contains("  ytdlp_extractor_args TEXT"));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&old).unwrap();
        conn.execute(
            "INSERT INTO downloads (id, url, filename, save_path, category_id, status) \
             VALUES ('old-1', 'https://example.com/f.bin', 'f.bin', 'C:\\dl\\f.bin', 'cat-video', 'complete')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pending_links (id, url, source, status) \
             VALUES ('old-p1', 'https://example.com/g.bin', 'browser', 'pending')",
            [],
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    // New settings fields carry their defaults.
    let s = store.get_settings().unwrap();
    assert!(s.platform_downloads_enabled);
    assert_eq!(s.ytdlp_extractor_args, None);
    // The old download row reads back with the new columns at
    // their defaults (no parent, not a direct yt-dlp row).
    let row = store.get_download("old-1").unwrap().expect("old row");
    assert_eq!(row.parent_id, None);
    assert_eq!(row.child_kind, None);
    assert_eq!(row.merge_status, None);
    assert_eq!(row.source_url, None);
    assert_eq!(row.format_id, None);
    assert!(!row.direct_ytdlp);
    // The old pending row reads back with empty resolved fields.
    let pending = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].filename, None);
    assert_eq!(pending[0].headers, None);
    assert!(!pending[0].fragmented);
    // Round 84 — the old row's filesize is NULL (the migration
    // adds the column without a value for pre-Round-84 rows).
    assert_eq!(pending[0].filesize, None);
    // A resolved row round-trips through the new columns.
    let resolved = store
        .enqueue_pending_link_resolved(
            "https://rr1---sn.googlevideo.com/videoplayback?id=22",
            Some("https://www.youtube.com/watch?v=x"),
            None,
            &ResolvedLink {
                filename: Some("My Video.mp4".to_string()),
                headers: Some(
                    "{\"User-Agent\": \"Mozilla/5.0\", \"Referer\": \"https://www.youtube.com/\"}"
                        .to_string(),
                ),
                source_url: Some("https://www.youtube.com/watch?v=x".to_string()),
                format_id: Some("22".to_string()),
                audio_url: None,
                fragmented: false,
                // Round 84 — the resolved size round-trips too.
                filesize: Some(1_000_000),
            },
        )
        .unwrap();
    let pending = store.list_pending_links(None).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[1].filename.as_deref(), Some("My Video.mp4"));
    assert_eq!(pending[1].format_id.as_deref(), Some("22"));
    assert_eq!(pending[1].filesize, Some(1_000_000));
    let back = store.get_pending_link(&resolved.id).unwrap().expect("row");
    assert_eq!(back.filename.as_deref(), Some("My Video.mp4"));
    assert_eq!(back.format_id.as_deref(), Some("22"));
    assert_eq!(
        back.source_url.as_deref(),
        Some("https://www.youtube.com/watch?v=x")
    );
    assert!(back.headers.as_deref().unwrap().contains("Mozilla/5.0"));
    assert!(!back.fragmented);
    // The new settings survive a save round-trip.
    let mut s2 = s.clone();
    s2.platform_downloads_enabled = false;
    s2.ytdlp_extractor_args = Some("youtube:player_client=android".to_string());
    store.update_settings(&s2).unwrap();
    let s3 = store.get_settings().unwrap();
    assert!(!s3.platform_downloads_enabled);
    assert_eq!(
        s3.ytdlp_extractor_args.as_deref(),
        Some("youtube:player_client=android")
    );
}

// -- host: resolved video_found -------------------------------------------

fn resolved_variant() -> VideoVariantInput {
    VideoVariantInput {
        url: "https://rr1---sn.googlevideo.com/videoplayback?id=22".to_string(),
        quality: Some("720p".to_string()),
        format: Some("MP4".to_string()),
        live: false,
        drm: false,
        note: None,
        resolved: true,
        filename: Some("My Video.mp4".to_string()),
        headers: Some(
            "{\"User-Agent\": \"Mozilla/5.0\", \"Referer\": \"https://www.youtube.com/\", \"Cookie\": \"SID=abc\"}"
                .to_string(),
        ),
        audio_url: Some("https://rr1---sn.googlevideo.com/videoplayback?id=140".to_string()),
        source_url: Some("https://www.youtube.com/watch?v=x".to_string()),
        format_id: Some("22".to_string()),
        fragmented: false,
        // Round 84 — the resolved size rides the variant.
        filesize: Some(1_000_000),
    }
}

#[test]
fn host_video_found_resolved_variant_stores_resolver_metadata() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let reply = handle_host_message(
        &store,
        &HostMessage::VideoFound {
            page_url: "https://www.youtube.com/watch?v=x".to_string(),
            variants: vec![resolved_variant()],
            cookies: vec![],
        },
    );
    assert!(reply.ok, "resolved video_found must succeed: {reply:?}");
    assert_eq!(reply.enqueued, Some(1));
    let pending = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(pending.len(), 1);
    let row = &pending[0];
    assert_eq!(row.source, "video");
    assert_eq!(row.filename.as_deref(), Some("My Video.mp4"));
    assert_eq!(row.format_id.as_deref(), Some("22"));
    assert_eq!(
        row.audio_url.as_deref(),
        Some("https://rr1---sn.googlevideo.com/videoplayback?id=140")
    );
    assert_eq!(
        row.source_url.as_deref(),
        Some("https://www.youtube.com/watch?v=x")
    );
    // The headers ride the row — but a Cookie key that survived
    // in the map is stripped (defence in depth: headers must
    // never carry a session past the cookie validation).
    let headers = row.headers.clone().unwrap_or_default();
    assert!(headers.contains("Mozilla/5.0"));
    assert!(headers.contains("youtube.com"));
    assert!(!headers.contains("SID=abc"));
    assert!(!headers.contains("Cookie"));
    assert!(!row.fragmented);
    // Round 84 — the resolved size rides the row too.
    assert_eq!(row.filesize, Some(1_000_000));
}

#[test]
fn host_video_found_resolved_variant_refused_when_platform_downloads_off() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut s = store.get_settings().unwrap();
    s.platform_downloads_enabled = false;
    store.update_settings(&s).unwrap();
    let reply = handle_host_message(
        &store,
        &HostMessage::VideoFound {
            page_url: "https://www.youtube.com/watch?v=x".to_string(),
            variants: vec![resolved_variant()],
            cookies: vec![],
        },
    );
    assert!(!reply.ok, "platform downloads off must refuse: {reply:?}");
    assert!(store
        .list_pending_links(Some("pending"))
        .unwrap()
        .is_empty());
}

#[test]
fn host_video_found_resolved_variant_still_rejects_a_bad_url() {
    // Resolved skips the extension check (googlevideo URLs have
    // no extension) but not the scheme check: a garbage URL is
    // still refused, not enqueued.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut bad = resolved_variant();
    bad.url = "not a url".to_string();
    let reply = handle_host_message(
        &store,
        &HostMessage::VideoFound {
            page_url: "https://www.youtube.com/watch?v=x".to_string(),
            variants: vec![bad],
            cookies: vec![],
        },
    );
    assert!(!reply.ok, "a bad resolved URL must be refused: {reply:?}");
}

// -- header parsing --------------------------------------------------------

#[test]
fn headers_from_json_parses_user_agent_and_referer_and_drops_cookie() {
    let h = DownloadManager::headers_from_json(Some(
        "{\"User-Agent\": \"Mozilla/5.0\", \"Referer\": \"https://www.youtube.com/\", \"Cookie\": \"SID=abc\", \"cookie\": \"other=1\", \"X-Ignored\": \"yes\"}",
    ));
    assert_eq!(h.user_agent.as_deref(), Some("Mozilla/5.0"));
    assert_eq!(h.referer.as_deref(), Some("https://www.youtube.com/"));
    assert!(!h.is_empty());
    // Garbage, empty and missing input all mean "no headers" —
    // the probe then falls back to the plain path.
    assert!(DownloadManager::headers_from_json(Some("{not json")).is_empty());
    assert!(DownloadManager::headers_from_json(Some("{}")).is_empty());
    assert!(DownloadManager::headers_from_json(None).is_empty());
}

// -- DASH merge with the system ffmpeg ------------------------------------

fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn merge_dash_files_merges_video_and_audio_with_system_ffmpeg() {
    if !ffmpeg_available() {
        // The merge is still covered on the VPS (the bundle ships
        // ffmpeg); a machine without ffmpeg cannot run it.
        return;
    }
    let dir = tmpdir();
    let video = dir.join("part.video.mp4");
    let audio = dir.join("part.audio.m4a");
    let out = dir.join("merged.mp4");
    // One second of silent video + one second of tone, the way a
    // DASH pair arrives (video-only + audio-only).
    let mk_video = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=128x72:rate=10",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            video.to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let mk_audio = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "aac",
            audio.to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    assert!(
        mk_video.map(|s| s.success()).unwrap_or(false),
        "ffmpeg must make the video part"
    );
    assert!(
        mk_audio.map(|s| s.success()).unwrap_or(false),
        "ffmpeg must make the audio part"
    );
    let video_len = std::fs::metadata(&video).unwrap().len();
    merge_dash_files(
        video.to_str().unwrap(),
        audio.to_str().unwrap(),
        out.to_str().unwrap(),
        Path::new("ffmpeg"),
    )
    .await
    .expect("the merge must succeed");
    let merged = std::fs::metadata(&out).expect("merged file");
    // The merged file holds both parts — bigger than the video
    // alone — and ffmpeg can read it back with two streams.
    assert!(merged.len() > video_len);
    let probe = std::process::Command::new("ffmpeg")
        .args(["-i", out.to_str().unwrap()])
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&probe.stderr);
    assert!(
        info.contains("Stream #0:0"),
        "merged file must have a video stream: {info}"
    );
    assert!(
        info.contains("Stream #0:1"),
        "merged file must have an audio stream: {info}"
    );
}

// -- Round 86 — the failure line's flattened tail -------------------------
// (Jumani) A mock yt-dlp that exits 1 must leave a failure line
// whose `tail:` is ONE line: the old multi-line tail split the
// log, so a grep for the reason missed it. The fake below is the
// same shape as the one in video_tests.rs (each test binary is
// its own process, so the lock cannot leak across binaries).

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
async fn round86_direct_ytdlp_failure_line_tail_is_single_line() {
    let _fake_guard = FAKE_YTDLP_LOCK.lock().unwrap();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(store, 4);
    let row = fetchline_engine::ytdlp::QualityRow {
        height: 360,
        label: "360p".to_string(),
        ext: "mp4".to_string(),
        has_audio: true,
        needs_merge: false,
        filesize: Some(512 * 1024),
        url: "https://example.com/video.mp4".to_string(),
        audio_url: None,
        audio_format_id: None,
        format_id: "18".to_string(),
        headers: std::collections::HashMap::new(),
        fragmented: false,
    };
    let (id, _) = mgr
        .add_resolved_video_download(
            "https://www.tiktok.com/@user/video/123",
            &row,
            Some("video.mp4".to_string()),
            None,
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
        "the mock exit 1 must fail the run, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    let tail_line = log
        .lines()
        .find(|l| l.contains("tail:"))
        .expect("the failure line must carry a tail:");
    assert!(
        tail_line.contains("direct yt-dlp download") && tail_line.contains("failed:"),
        "the tail: must sit on the failure line itself: {tail_line}"
    );
    assert!(
        tail_line.contains("Requested format is not available"),
        "the flattened tail must carry the stderr reason: {tail_line}"
    );
    assert_eq!(
        log.lines().filter(|l| l.contains("ERROR:")).count(),
        1,
        "the stderr reason must appear exactly once, on the tail line — never as a bare multi-line dump: {log}"
    );
}
