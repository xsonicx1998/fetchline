//! Update check ("Quick update check", S09 / should-have) tests.
//! Every test serves its own version manifest from a local
//! fixture server on 127.0.0.1 — no real update server, no IDM
//! servers, no public internet is ever contacted.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fetchline_engine::timeutil::{now_unix, unix_to_iso};
use fetchline_engine::{DownloadManager, Store};

/// A tiny fixture update server: serves one configurable
/// manifest body at any path, counts requests, and can be told
/// to answer with a non-2xx status instead.
struct ManifestServer {
    url: String,
    hits: Arc<AtomicUsize>,
    state: Arc<Mutex<(u16, String)>>,
}

impl ManifestServer {
    fn start(status: u16, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(Mutex::new((status, body.to_string())));
        let h = Arc::clone(&hits);
        let s = Arc::clone(&state);
        std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        h.fetch_add(1, Ordering::SeqCst);
                        let mut buf = [0u8; 4096];
                        let _ = stream.read(&mut buf);
                        let (status, body) = s.lock().unwrap().clone();
                        let reason = if status == 200 { "OK" } else { "Error" };
                        let _ = stream.write_all(
                            format!(
                                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        );
                        let _ = stream.flush();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}/latest.json"),
            hits,
            state,
        }
    }

    fn set(&self, status: u16, body: &str) {
        *self.state.lock().unwrap() = (status, body.to_string());
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn tmpdir() -> std::path::PathBuf {
    // BUG-041: the counter disambiguates two tests that call this
    // in the same nanosecond tick (parallel tests in one binary
    // share the PID, and the Windows clock tick can repeat a
    // nanos value) — without it two tests opened the SAME fresh
    // app.db and the loser failed Store::open with "database is
    // locked". Same shape as common::tmpdir (BUG-040).
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "fetchline-test-update-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst),
        now_unix_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn now_unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

/// Point the manager's update URL at the fixture server.
fn point_at(mgr: &DownloadManager, url: &str) {
    let mut settings = mgr.get_settings().unwrap();
    settings.update_url = url.to_string();
    mgr.update_settings(&settings).unwrap();
}

const NEWER_MANIFEST: &str = r#"{"version": "0.2.0", "changelog": ["Faster segmented downloads", "Fixed a crash when pausing"], "download_url": "https://updates.fetchline.app/releases/fetchline-0.2.0.msi"}"#;

#[tokio::test]
async fn newer_version_available_shows_changelog_and_never_installs() {
    let dir = tmpdir();
    let server = ManifestServer::start(200, NEWER_MANIFEST);
    let mgr = test_mgr(&dir);
    point_at(&mgr, &server.url);

    let result = mgr.check_for_updates().await.unwrap();
    assert!(result.update_available, "0.2.0 must be newer than 0.1.0");
    assert_eq!(result.current_version, "0.1.0");
    assert_eq!(result.latest_version, "0.2.0");
    assert_eq!(
        result.changelog,
        vec!["Faster segmented downloads", "Fixed a crash when pausing"]
    );
    assert_eq!(
        result.download_url.as_deref(),
        Some("https://updates.fetchline.app/releases/fetchline-0.2.0.msi")
    );
    // Ask before updating: the check itself installs nothing and
    // starts no download — the list is still empty, and exactly
    // one request (the manifest GET) was made.
    assert!(mgr.list_downloads().unwrap().is_empty());
    assert_eq!(server.hits(), 1);
    // The updater plugin is not configured in this build, so the
    // result says so instead of pretending an install exists.
    assert!(!result.updater_configured);
    // The check is recorded, so the weekly timer knows it ran.
    let settings = mgr.get_settings().unwrap();
    assert!(settings.last_update_check_at.is_some());
    assert_eq!(result.checked_at, settings.last_update_check_at.unwrap());
}

#[tokio::test]
async fn up_to_date_reports_no_update_and_sends_no_prompt() {
    let dir = tmpdir();
    let server = ManifestServer::start(200, r#"{"version": "0.1.0", "changelog": []}"#);
    let mgr = test_mgr(&dir);
    point_at(&mgr, &server.url);
    let mut rx = mgr.subscribe_update_checks();

    let result = mgr.run_weekly_update_check_at(now_unix()).await.unwrap();
    let result = result.expect("a never-checked install is due a weekly check");
    assert!(!result.update_available, "same version is not an update");
    assert!(result.changelog.is_empty());
    // No prompt: the weekly channel stays silent when there is
    // nothing new.
    assert!(rx.try_recv().is_err());
    // An older manifest is not an update either.
    server.set(200, r#"{"version": "0.0.9", "changelog": ["Old"]}"#);
    let mut settings = mgr.get_settings().unwrap();
    settings.last_update_check_at = Some(unix_to_iso(now_unix() - 8 * 24 * 3600));
    mgr.update_settings(&settings).unwrap();
    let result = mgr
        .run_weekly_update_check_at(now_unix())
        .await
        .unwrap()
        .unwrap();
    assert!(!result.update_available);
    // A pre-release of the current version is older, not newer.
    server.set(200, r#"{"version": "0.1.0-beta", "changelog": []}"#);
    let mut settings = mgr.get_settings().unwrap();
    settings.last_update_check_at = Some(unix_to_iso(now_unix() - 8 * 24 * 3600));
    mgr.update_settings(&settings).unwrap();
    let result = mgr
        .run_weekly_update_check_at(now_unix())
        .await
        .unwrap()
        .unwrap();
    assert!(!result.update_available);
}

#[tokio::test]
async fn malformed_manifest_is_a_clear_error_not_a_crash() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    for (body, needle) in [
        ("{not json", "could not read"),
        (r#"{"changelog": []}"#, "no version"),
        (r#"{"version": ""}"#, "no version"),
        (r#"{"version": "banana"}"#, "cannot read"),
        (
            r#"{"version": "0.2.0", "download_url": "ftp://example.com/x.msi"}"#,
            "cannot use",
        ),
    ] {
        let server = ManifestServer::start(200, body);
        let err = mgr.check_for_updates_from(&server.url).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(needle),
            "manifest {body} should fail with '{needle}', got: {msg}"
        );
    }
    // Even a failed check is recorded, so the weekly timer
    // retries next week instead of hammering the server.
    assert!(mgr.get_settings().unwrap().last_update_check_at.is_some());
}

#[tokio::test]
async fn network_failure_and_server_error_are_clear_errors() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // A closed port: no server at all.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let err = mgr
        .check_for_updates_from(&format!("http://127.0.0.1:{port}/latest.json"))
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Could not reach the update server"),
        "got: {err}"
    );
    // A server that answers 500.
    let server = ManifestServer::start(500, "boom");
    let err = mgr.check_for_updates_from(&server.url).await.unwrap_err();
    assert!(err.to_string().contains("status 500"), "got: {err}");
    // A non-http(s) update URL is refused before any request.
    let err = mgr
        .check_for_updates_from("ftp://127.0.0.1/latest.json")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("http:// or https://"),
        "got: {err}"
    );
}

#[tokio::test]
async fn weekly_setting_off_skips_automatic_check_but_manual_still_works() {
    let dir = tmpdir();
    let server = ManifestServer::start(200, NEWER_MANIFEST);
    let mgr = test_mgr(&dir);
    point_at(&mgr, &server.url);
    let mut settings = mgr.get_settings().unwrap();
    settings.check_updates_weekly = false;
    mgr.update_settings(&settings).unwrap();

    // No automatic check: nothing is fetched, no result.
    assert!(!mgr.weekly_update_check_due_at(now_unix()).unwrap());
    let ran = mgr.run_weekly_update_check_at(now_unix()).await.unwrap();
    assert!(ran.is_none());
    assert_eq!(server.hits(), 0);

    // The manual check still works with the weekly setting off.
    let result = mgr.check_for_updates().await.unwrap();
    assert!(result.update_available);
    assert_eq!(server.hits(), 1);
}

#[tokio::test]
async fn weekly_check_runs_when_due_and_not_before_and_notifies_on_update() {
    let dir = tmpdir();
    let server = ManifestServer::start(200, NEWER_MANIFEST);
    let mgr = test_mgr(&dir);
    point_at(&mgr, &server.url);
    let mut rx = mgr.subscribe_update_checks();

    // Never checked: due. The weekly run finds the update and
    // the UI channel carries it, so the dialog can ask the user.
    let ran = mgr.run_weekly_update_check_at(now_unix()).await.unwrap();
    assert!(ran.is_some() && ran.unwrap().update_available);
    assert_eq!(server.hits(), 1);
    let notified = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the weekly check must notify the UI")
        .expect("the channel must carry the result");
    assert_eq!(notified.latest_version, "0.2.0");

    // Just checked: not due, no second fetch.
    let ran = mgr.run_weekly_update_check_at(now_unix()).await.unwrap();
    assert!(ran.is_none());
    assert_eq!(server.hits(), 1);

    // Checked 6 days ago: still not due.
    let mut settings = mgr.get_settings().unwrap();
    settings.last_update_check_at = Some(unix_to_iso(now_unix() - 6 * 24 * 3600));
    mgr.update_settings(&settings).unwrap();
    assert!(!mgr.weekly_update_check_due_at(now_unix()).unwrap());
    assert!(mgr
        .run_weekly_update_check_at(now_unix())
        .await
        .unwrap()
        .is_none());
    assert_eq!(server.hits(), 1);

    // Checked 8 days ago: due again, and it fetches again.
    let mut settings = mgr.get_settings().unwrap();
    settings.last_update_check_at = Some(unix_to_iso(now_unix() - 8 * 24 * 3600));
    mgr.update_settings(&settings).unwrap();
    assert!(mgr.weekly_update_check_due_at(now_unix()).unwrap());
    let ran = mgr.run_weekly_update_check_at(now_unix()).await.unwrap();
    assert!(ran.is_some());
    assert_eq!(server.hits(), 2);
}

#[tokio::test]
async fn scheduler_tick_spawns_the_weekly_check_on_the_existing_tick() {
    let dir = tmpdir();
    let server = ManifestServer::start(200, NEWER_MANIFEST);
    let mgr = test_mgr(&dir);
    point_at(&mgr, &server.url);
    // No second scheduler: the 30 s tick is the trigger.
    mgr.scheduler_tick().await.unwrap();
    let mut fetched = false;
    for _ in 0..50 {
        if server.hits() >= 1 {
            fetched = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(fetched, "the tick must spawn the due weekly check");
    // The check recorded itself, so the next tick does not
    // immediately spawn another one.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.hits(), 1);
    assert!(mgr.get_settings().unwrap().last_update_check_at.is_some());
}

#[tokio::test]
async fn update_settings_persist_validate_and_migrate() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // The default is Fetchline's own placeholder, never IDM's
    // servers or a third-party feed.
    let settings = mgr.get_settings().unwrap();
    assert_eq!(
        settings.update_url,
        "https://updates.fetchline.app/releases/latest.json"
    );
    assert!(settings.check_updates_weekly);
    assert!(settings.last_update_check_at.is_none());

    // A custom server round-trips and applies live (a manual
    // check then uses it — one request, no restart).
    let server = ManifestServer::start(200, NEWER_MANIFEST);
    let mut settings = mgr.get_settings().unwrap();
    settings.update_url = server.url.clone();
    mgr.update_settings(&settings).unwrap();
    assert_eq!(mgr.get_settings().unwrap().update_url, server.url);
    assert!(mgr.check_for_updates().await.unwrap().update_available);
    assert_eq!(server.hits(), 1);

    // Bad addresses are rejected, nothing stored.
    for bad in [
        "",
        "not a url",
        "ftp://updates.fetchline.app/x.json",
        "updates.fetchline.app/x.json",
    ] {
        let mut settings = mgr.get_settings().unwrap();
        settings.update_url = bad.to_string();
        let err = mgr.update_settings(&settings).unwrap_err();
        assert!(
            err.to_string().contains("http:// or https://"),
            "{bad} should be rejected, got: {err}"
        );
        assert_eq!(mgr.get_settings().unwrap().update_url, server.url);
    }
}

#[test]
fn update_columns_migrate_on_old_databases() {
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        // A database from before the update check: settings
        // without the update columns.
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
             INSERT INTO settings (id) VALUES (1);",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let s = store.get_settings().unwrap();
    assert_eq!(
        s.update_url,
        "https://updates.fetchline.app/releases/latest.json"
    );
    assert!(s.last_update_check_at.is_none());
    // And the new fields can be saved into the migrated database.
    let mut s2 = s.clone();
    s2.update_url = "http://127.0.0.1:9/latest.json".to_string();
    store.update_settings(&s2).unwrap();
    assert_eq!(
        store.get_settings().unwrap().update_url,
        "http://127.0.0.1:9/latest.json"
    );
}
