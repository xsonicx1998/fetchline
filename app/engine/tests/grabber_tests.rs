//! Site grabber (S06 / F06) tests: crawl rules, filters, caps,
//! robots, login walls, the 2-attempt rule, excluded sites, and
//! explore-only mode. Every test serves its own fixture site on
//! 127.0.0.1 (`GrabberSite` in `common/mock.rs`) — no real website
//! is ever crawled.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use mock::*;
use std::sync::Arc;
use std::time::Duration;

use fetchline_engine::timeutil::now_unix;
use fetchline_engine::{DownloadManager, GrabberProjectRow, Store};

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

fn all_types() -> Vec<String> {
    ["image", "video", "audio", "document", "archive", "program"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn types(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// Poll a project until it reaches a terminal status
/// (done / failed / stopped), or panic with the last state seen.
async fn wait_grabber(mgr: &DownloadManager, id: &str, secs: u64) -> GrabberProjectRow {
    let mut last: Option<GrabberProjectRow> = None;
    for _ in 0..secs * 10 {
        if let Ok(Some(p)) = mgr.get_grabber_project(id) {
            if matches!(p.status.as_str(), "done" | "failed" | "stopped") {
                return p;
            }
            last = Some(p);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("grabber {id} did not finish in {secs}s; last state: {last:?}")
}

fn matched_urls(p: &GrabberProjectRow) -> Vec<String> {
    p.matched_files.iter().map(|f| f.url.clone()).collect()
}

// -- creation validation --------------------------------------------------

#[test]
fn create_rejects_depth_over_5_empty_types_and_bad_url() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let err = mgr
        .create_grabber_project(
            "p",
            "http://127.0.0.1/",
            &types(&["image"]),
            6,
            200,
            "download",
            None,
        )
        .unwrap_err();
    assert!(err.to_string().contains("too deep"), "{err}");
    assert!(mgr
        .create_grabber_project("p", "http://127.0.0.1/", &[], 2, 200, "download", None)
        .is_err());
    assert!(mgr
        .create_grabber_project(
            "p",
            "http://127.0.0.1/",
            &types(&["image"]),
            2,
            1001,
            "download",
            None
        )
        .is_err());
    assert!(mgr
        .create_grabber_project(
            "p",
            "not a url",
            &types(&["image"]),
            2,
            200,
            "download",
            None
        )
        .is_err());
    assert!(mgr
        .create_grabber_project(
            "",
            "http://127.0.0.1/",
            &types(&["image"]),
            2,
            200,
            "download",
            None
        )
        .is_err());
    let ok = mgr
        .create_grabber_project(
            "p",
            "http://127.0.0.1/",
            &types(&["image", "document"]),
            2,
            200,
            "download",
            None,
        )
        .unwrap();
    assert_eq!(ok.status, "idle");
    assert_eq!(
        ok.filters.types,
        vec!["image".to_string(), "document".to_string()]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn create_rejects_excluded_start_site() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    set_excluded(mgr.store(), &["127.0.0.1"]);
    let err = mgr
        .create_grabber_project(
            "p",
            "http://127.0.0.1/page.html",
            &types(&["image"]),
            2,
            200,
            "download",
            None,
        )
        .unwrap_err();
    assert!(err.to_string().contains("excluded"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

// -- explore-only ---------------------------------------------------------

#[tokio::test]
async fn explore_only_lists_matched_files_without_downloading() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project(
            "explore",
            &site.page_url,
            &all_types(),
            2,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.pages_explored, 4); // index, page2, page3, deep
    assert_eq!(done.files_matched, 8); // pic, doc, video, song, exe, notes, dead, archive
    assert_eq!(done.files_downloaded, 0);
    assert_eq!(done.files_failed, 0);
    let urls = matched_urls(&done);
    assert!(urls.iter().any(|u| u.ends_with("/pic.png")));
    assert!(urls.iter().any(|u| u.ends_with("/archive.zip")));
    assert!(!urls.iter().any(|u| u.ends_with("/secret.pdf"))); // depth 3, not reached
                                                               // BUG-040: print the rows on failure — a non-empty list here
                                                               // used to mean the test opened a stale tmpdir database, and
                                                               // the bare `is_empty()` gave no clue which rows leaked in.
    let downloads = mgr.list_downloads().unwrap();
    assert!(
        downloads.is_empty(),
        "explore-only must not download, but the list holds: {downloads:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn same_host_only_off_site_link_never_matched_or_followed() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project(
            "host",
            &site.page_url,
            &all_types(),
            5,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert!(matched_urls(&done).iter().all(|u| u.contains("127.0.0.1")));
    assert!(!matched_urls(&done).iter().any(|u| u.contains("evil.png")));
    assert_eq!(done.pages_explored, 5); // index, page2, page3, deep, deeper
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn depth_limit_is_enforced() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    // Depth 0: the start page only.
    let p0 = mgr
        .create_grabber_project("d0", &site.page_url, &all_types(), 0, 200, "explore", None)
        .unwrap();
    mgr.run_grabber(&p0.id).await.unwrap();
    let done0 = wait_grabber(&mgr, &p0.id, 30).await;
    assert_eq!(done0.pages_explored, 1);
    assert_eq!(done0.files_matched, 3); // pic, doc, video
                                        // Depth 1: page2/page3 files appear, deep files do not.
    let p1 = mgr
        .create_grabber_project("d1", &site.page_url, &all_types(), 1, 200, "explore", None)
        .unwrap();
    mgr.run_grabber(&p1.id).await.unwrap();
    let done1 = wait_grabber(&mgr, &p1.id, 30).await;
    assert_eq!(done1.pages_explored, 3);
    assert!(matched_urls(&done1)
        .iter()
        .any(|u| u.ends_with("/song.mp3")));
    assert!(!matched_urls(&done1)
        .iter()
        .any(|u| u.ends_with("/archive.zip")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn type_filter_only_matches_selected_types() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project(
            "img",
            &site.page_url,
            &types(&["image"]),
            2,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_matched, 1);
    assert!(done.matched_files[0].url.ends_with("/pic.png"));
    assert_eq!(done.matched_files[0].kind, "image");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn max_pages_cap_stops_cleanly() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project("cap", &site.page_url, &all_types(), 5, 2, "explore", None)
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(
        done.status, "done",
        "the cap is a clean stop, not a failure"
    );
    assert_eq!(done.pages_explored, 2);
    // Only the first two pages (index, then page2) contributed files.
    assert_eq!(done.files_matched, 4); // pic, doc, video, song
    let _ = std::fs::remove_dir_all(&dir);
}

// -- robots / login / dead start page --------------------------------------

#[tokio::test]
async fn robots_disallow_blocks_the_start_url() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::BlockAll);
    let p = mgr
        .create_grabber_project(
            "robots",
            &site.page_url,
            &all_types(),
            2,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "failed");
    let err = done.error_message.unwrap_or_default();
    assert!(err.contains("robots.txt"), "{err}");
    assert_eq!(done.pages_explored, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn login_required_start_page_is_reported_not_skipped() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project(
            "login",
            &site.login_url,
            &all_types(),
            2,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "failed");
    let err = done.error_message.unwrap_or_default();
    assert!(err.contains("login"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn dead_start_page_fails_with_the_status() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let missing = format!("{}/missing.html", site.base);
    let p = mgr
        .create_grabber_project("dead", &missing, &all_types(), 2, 200, "explore", None)
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "failed");
    assert!(done.error_message.unwrap_or_default().contains("404"));
    let _ = std::fs::remove_dir_all(&dir);
}

// -- download mode: real downloads, 2 attempts, tagging --------------------

#[tokio::test]
async fn download_mode_downloads_matched_files_and_tags_them() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project(
            "dl",
            &site.page_url,
            &types(&["image", "document"]),
            1,
            200,
            "download",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_matched, 4); // pic, doc, notes, dead.pdf
    assert_eq!(done.files_downloaded, 3);
    assert_eq!(done.files_failed, 1); // dead.pdf fails its probe twice
    let downloads = mgr.list_downloads().unwrap();
    assert_eq!(downloads.len(), 3);
    assert!(downloads.iter().all(|d| d.status == "complete"));
    // Every grabbed file is tagged with the project in the database.
    let db = rusqlite::Connection::open(dir.join("app.db")).unwrap();
    let tagged: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE grabber_project_id = ?1",
            [p.id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tagged, 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn two_attempts_then_the_grabber_gives_up_on_a_dead_file() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    // Depth 1, documents only: doc.pdf + notes.txt download, the
    // dead.pdf on page3 fails its probe twice and is recorded once.
    let p = mgr
        .create_grabber_project(
            "flaky",
            &site.page_url,
            &types(&["document"]),
            1,
            200,
            "download",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "done", "one dead file must not fail the run");
    assert_eq!(done.files_matched, 3);
    assert_eq!(done.files_downloaded, 2);
    assert_eq!(done.files_failed, 1);
    let dead = done
        .matched_files
        .iter()
        .find(|f| f.url.ends_with("/dead.pdf"))
        .expect("dead.pdf should be listed");
    assert_eq!(dead.status, "failed");
    assert!(dead.error.as_deref().unwrap_or_default().contains("404"));
    let _ = std::fs::remove_dir_all(&dir);
}

// -- stop + schedule --------------------------------------------------------

#[tokio::test]
async fn stop_grabber_records_stopped() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project(
            "stop",
            &site.page_url,
            &all_types(),
            5,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    // The crawl takes ~2 s (500 ms between 5 pages); stop mid-crawl.
    tokio::time::sleep(Duration::from_millis(600)).await;
    mgr.stop_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "stopped");
    assert!(done.pages_explored < 5, "the run stopped before finishing");
    // A stopped grabber is not running any more.
    assert!(mgr.stop_grabber(&p.id).await.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn scheduled_project_starts_on_the_scheduler_tick() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project(
            "sched",
            &site.page_url,
            &types(&["image"]),
            0,
            200,
            "explore",
            Some("2026-01-01T00:00:00Z"),
        )
        .unwrap();
    assert_eq!(p.status, "idle");
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_matched, 1);
    // A future schedule does not start yet.
    let future = mgr
        .create_grabber_project(
            "future",
            &site.page_url,
            &types(&["image"]),
            0,
            200,
            "explore",
            Some("2999-01-01T00:00:00Z"),
        )
        .unwrap();
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    let still = mgr.get_grabber_project(&future.id).unwrap().unwrap();
    assert_eq!(still.status, "idle");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn run_grabber_twice_is_rejected_while_running() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project(
            "twice",
            &site.page_url,
            &all_types(),
            5,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    assert!(mgr.run_grabber(&p.id).await.is_err());
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    // After finishing, a re-run is allowed.
    mgr.run_grabber(&p.id).await.unwrap();
    let done2 = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done2.status, "done");
    let _ = std::fs::remove_dir_all(&dir);
}

// -- Milestone 4: per-project queue choice + on-completion action ------

/// Create a project with a queue + completion action (Milestone 4).
#[allow(clippy::too_many_arguments)]
fn create_full(
    mgr: &DownloadManager,
    name: &str,
    start_url: &str,
    types: Vec<String>,
    depth: i64,
    mode: &str,
    queue_id: Option<&str>,
    action: &str,
    program: Option<&str>,
) -> GrabberProjectRow {
    mgr.create_grabber_project_full(
        name, start_url, &types, depth, 200, mode, None, queue_id, action, program,
    )
    .unwrap()
}

/// A shell script that proves it ran by writing a marker file
/// (same pattern as the queue run-program test in engine_tests.rs).
/// Windows runs a .bat directly (CreateProcess handles it); Unix
/// runs a .sh with +x.
fn marker_script(dir: &std::path::Path, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let marker = dir.join(format!("{name}.ran.txt"));
    #[cfg(windows)]
    let script = dir.join(format!("{name}.bat"));
    #[cfg(not(windows))]
    let script = dir.join(format!("{name}.sh"));
    #[cfg(windows)]
    std::fs::write(&script, format!("@echo done > \"{}\"\n", marker.display())).unwrap();
    #[cfg(not(windows))]
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho done > {}\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    (script, marker)
}

#[tokio::test]
async fn grabber_files_land_in_the_chosen_queue_not_main() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let queue = mgr.create_queue("Photos").unwrap();
    let p = create_full(
        &mgr,
        "chosen",
        &site.page_url,
        types(&["image", "document"]),
        1,
        "download",
        Some(&queue.id),
        "none",
        None,
    );
    assert_eq!(p.queue_id, queue.id);
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_downloaded, 3);
    let in_queue = mgr.list_queue_files(&queue.id).unwrap();
    assert_eq!(
        in_queue.len(),
        3,
        "every grabbed file must be filed in the chosen queue"
    );
    assert!(in_queue.iter().all(|d| d.status == "complete"));
    let in_main = mgr.list_queue_files("queue-main").unwrap();
    assert!(in_main.is_empty(), "nothing may leak into the main queue");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn grabber_create_rejects_unknown_queue() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let err = mgr
        .create_grabber_project_full(
            "p",
            "http://127.0.0.1/",
            &types(&["image"]),
            2,
            200,
            "download",
            None,
            Some("queue-nope"),
            "none",
            None,
        )
        .unwrap_err();
    assert!(err.to_string().contains("Queue not found"), "{err}");
    assert!(
        mgr.list_grabber_projects().unwrap().is_empty(),
        "nothing is saved on a rejected create"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn grabber_create_defaults_to_main_queue_and_no_action() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = mgr
        .create_grabber_project(
            "plain",
            &site.page_url,
            &types(&["image"]),
            0,
            200,
            "download",
            None,
        )
        .unwrap();
    assert_eq!(p.queue_id, "queue-main");
    assert_eq!(p.on_complete_action, "none");
    assert_eq!(p.on_complete_program, None);
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_downloaded, 1);
    let in_main = mgr.list_queue_files("queue-main").unwrap();
    assert_eq!(in_main.len(), 1);
    assert!(mgr.pending_action("queue-main").await.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn grabber_create_rejects_bad_completion_action() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let err = mgr
        .create_grabber_project_full(
            "p",
            "http://127.0.0.1/",
            &types(&["image"]),
            2,
            200,
            "download",
            None,
            None,
            "explode",
            None,
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("Unknown completion action"),
        "{err}"
    );
    for bad_program in [None, Some(""), Some("   ")] {
        let err = mgr
            .create_grabber_project_full(
                "p",
                "http://127.0.0.1/",
                &types(&["image"]),
                2,
                200,
                "download",
                None,
                None,
                "run_program",
                bad_program,
            )
            .unwrap_err();
        assert!(err.to_string().contains("Choose a program"), "{err}");
    }
    // shutdown / hibernate need no program and are accepted.
    let ok = create_full(
        &mgr,
        "shut",
        "http://127.0.0.1/",
        types(&["image"]),
        2,
        "download",
        None,
        "shutdown",
        None,
    );
    assert_eq!(ok.on_complete_action, "shutdown");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn grabber_run_fails_when_its_queue_is_gone() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let queue = mgr.create_queue("Doomed").unwrap();
    let p = create_full(
        &mgr,
        "gone",
        &site.page_url,
        types(&["image"]),
        0,
        "download",
        Some(&queue.id),
        "none",
        None,
    );
    // The queue is deleted after the project was created — the
    // create-time check cannot catch this, the run must.
    {
        let db = rusqlite::Connection::open(dir.join("app.db")).unwrap();
        db.execute("DELETE FROM queues WHERE id = ?1", [queue.id.as_str()])
            .unwrap();
    }
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "failed");
    let err = done.error_message.unwrap_or_default();
    assert!(err.contains("no longer exists"), "{err}");
    assert_eq!(
        done.pages_explored, 0,
        "the run must fail before crawling anything"
    );
    // BUG-040: print the rows on failure — a non-empty list here
    // used to mean the test opened a stale tmpdir database, and
    // the bare `is_empty()` gave no clue which rows leaked in.
    let downloads = mgr.list_downloads().unwrap();
    assert!(
        downloads.is_empty(),
        "no downloads may exist when the run failed before crawling, but the list holds: {downloads:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn grabber_completion_action_runs_when_files_downloaded() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let (script, marker) = marker_script(&dir, "grabber_done");
    let p = create_full(
        &mgr,
        "act",
        &site.page_url,
        types(&["image"]),
        0,
        "download",
        None,
        "run_program",
        Some(script.to_string_lossy().as_ref()),
    );
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_downloaded, 1);
    let pending = mgr
        .pending_action("queue-main")
        .await
        .expect("the action must be armed after downloading a file");
    assert_eq!(pending.action, "run_program");
    assert!(
        pending
            .file_path
            .as_deref()
            .unwrap_or_default()
            .ends_with("pic.png"),
        "the action carries the downloaded file: {:?}",
        pending.file_path
    );
    assert!(!marker.exists());
    mgr.execute_queue_action("queue-main").await.unwrap();
    for _ in 0..50 {
        if marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(marker.exists(), "the program must have run");
    assert!(mgr.pending_action("queue-main").await.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn grabber_completion_action_not_armed_when_nothing_downloaded() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    // Only-dead page: one matched file, it fails both attempts.
    let dead_page = format!("{}/only-dead.html", site.base);
    let p = create_full(
        &mgr,
        "zero",
        &dead_page,
        types(&["document"]),
        0,
        "download",
        None,
        "run_program",
        Some("/bin/true"),
    );
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "done", "one dead file must not fail the run");
    assert_eq!(done.files_matched, 1);
    assert_eq!(done.files_downloaded, 0);
    assert_eq!(done.files_failed, 1);
    assert!(
        mgr.pending_action("queue-main").await.is_none(),
        "no download, no action — the BUG-003 rule"
    );
    // And a run that matched nothing at all must not arm it either.
    let p2 = create_full(
        &mgr,
        "zero2",
        &dead_page,
        types(&["image"]),
        0,
        "download",
        None,
        "shutdown",
        None,
    );
    mgr.run_grabber(&p2.id).await.unwrap();
    let done2 = wait_grabber(&mgr, &p2.id, 60).await;
    assert_eq!(done2.status, "done");
    assert_eq!(done2.files_matched, 0);
    assert_eq!(done2.files_downloaded, 0);
    assert!(mgr.pending_action("queue-main").await.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn grabber_completion_action_not_armed_in_explore_only() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = create_full(
        &mgr,
        "exp",
        &site.page_url,
        all_types(),
        2,
        "explore",
        None,
        "shutdown",
        None,
    );
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_matched, 8);
    assert_eq!(done.files_downloaded, 0);
    // BUG-040: print the rows on failure — a non-empty list here
    // used to mean the test opened a stale tmpdir database, and
    // the bare `is_empty()` gave no clue which rows leaked in.
    let downloads = mgr.list_downloads().unwrap();
    assert!(
        downloads.is_empty(),
        "explore-only downloaded nothing, but the list holds: {downloads:?}"
    );
    assert!(
        mgr.pending_action("queue-main").await.is_none(),
        "explore-only downloaded nothing, so nothing may be armed"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn grabber_completion_action_not_armed_when_the_run_failed() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let missing = format!("{}/missing.html", site.base);
    let p = create_full(
        &mgr,
        "dead",
        &missing,
        all_types(),
        2,
        "download",
        None,
        "shutdown",
        None,
    );
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "failed");
    assert_eq!(done.files_downloaded, 0);
    assert!(
        mgr.pending_action("queue-main").await.is_none(),
        "a failed run must not arm its action"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn grabber_completion_action_cancel_clears_it() {
    // The cancel path of the 60 s countdown (engine side): dismiss
    // clears the pending action, and nothing runs afterwards. The
    // shutdown itself is NEVER executed in a test.
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = create_full(
        &mgr,
        "cancel",
        &site.page_url,
        types(&["image"]),
        0,
        "download",
        None,
        "shutdown",
        None,
    );
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_downloaded, 1);
    let pending = mgr
        .pending_action("queue-main")
        .await
        .expect("the shutdown action must be armed after downloading a file");
    assert_eq!(pending.action, "shutdown");
    mgr.dismiss_queue_action("queue-main").await.unwrap();
    assert!(mgr.pending_action("queue-main").await.is_none());
    // Cancelling means nothing runs: executing now must fail.
    assert!(mgr.execute_queue_action("queue-main").await.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn grabber_completion_action_open_file_names_the_last_file() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = create_full(
        &mgr,
        "open",
        &site.page_url,
        types(&["image", "document"]),
        1,
        "download",
        None,
        "open_file",
        None,
    );
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_downloaded, 3);
    let pending = mgr
        .pending_action("queue-main")
        .await
        .expect("the open-file action must be armed");
    assert_eq!(pending.action, "open_file");
    assert!(
        pending
            .file_path
            .as_deref()
            .unwrap_or_default()
            .ends_with("notes.txt"),
        "the last file downloaded is notes.txt: {:?}",
        pending.file_path
    );
    // Deliberately NOT executed: it would spawn the desktop opener.
    mgr.dismiss_queue_action("queue-main").await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn grabber_completion_action_not_armed_when_stopped_after_downloading() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = GrabberSite::start(RobotsMode::None);
    let p = create_full(
        &mgr,
        "stopdl",
        &site.page_url,
        all_types(),
        5,
        "download",
        None,
        "shutdown",
        None,
    );
    mgr.run_grabber(&p.id).await.unwrap();
    // Poll fast: stop the moment the download phase has finished at
    // least one file, while files remain — mirroring the queue
    // drain, a stopped run arms no action.
    let mut stopped = false;
    for _ in 0..600 {
        if let Ok(Some(cur)) = mgr.get_grabber_project(&p.id) {
            if cur.status == "downloading" && cur.files_downloaded >= 1 {
                if mgr.stop_grabber(&p.id).await.is_ok() {
                    stopped = true;
                    break;
                }
            }
            if matches!(cur.status.as_str(), "done" | "failed" | "stopped") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        stopped,
        "the stop must land mid-download, not after the run finished"
    );
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "stopped");
    assert!(
        done.files_downloaded >= 1,
        "files did download before the stop"
    );
    assert!(
        mgr.pending_action("queue-main").await.is_none(),
        "a stopped run must not arm its action even though files downloaded"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pre_milestone4_database_migrates_grabber_projects() {
    // A database made before Milestone 4 has a grabber_projects
    // table without queue_id / on_complete_action /
    // on_complete_program. Opening it must add them (Store::migrate)
    // without losing the project, and the old project must read
    // back as main queue + no action.
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE grabber_projects (
               id TEXT PRIMARY KEY, name TEXT NOT NULL, start_url TEXT NOT NULL,
               filters_json TEXT NOT NULL DEFAULT '{}',
               mode TEXT NOT NULL DEFAULT 'download'
                 CHECK (mode IN ('explore','download')),
               schedule_json TEXT,
               status TEXT NOT NULL DEFAULT 'idle'
                 CHECK (status IN ('idle','exploring','downloading','done','failed','stopped')),
               pages_explored INTEGER NOT NULL DEFAULT 0,
               files_matched INTEGER NOT NULL DEFAULT 0,
               files_downloaded INTEGER NOT NULL DEFAULT 0,
               files_failed INTEGER NOT NULL DEFAULT 0,
               error_message TEXT,
               matched_json TEXT NOT NULL DEFAULT '[]',
               created_at TEXT NOT NULL DEFAULT (datetime('now')),
               updated_at TEXT NOT NULL DEFAULT (datetime('now')));
             INSERT INTO grabber_projects (id, name, start_url, filters_json, mode, status)
               VALUES ('g-old', 'Old project', 'http://127.0.0.1/',
                 '{\"types\":[\"image\"],\"depth\":2,\"max_pages\":200}', 'explore', 'idle');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let projects = store.list_grabber_projects().unwrap();
    let old = projects
        .iter()
        .find(|p| p.id == "g-old")
        .expect("the old project must survive the migration");
    assert_eq!(old.name, "Old project");
    assert_eq!(old.queue_id, "queue-main");
    assert_eq!(old.on_complete_action, "none");
    assert_eq!(old.on_complete_program, None);
    let _ = std::fs::remove_dir_all(&dir);
}
