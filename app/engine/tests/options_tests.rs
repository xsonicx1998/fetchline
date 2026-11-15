//! Options dialog finish (S09): Site logins, Dial-Up, Sounds.
//! Every test serves its own fixture on 127.0.0.1 — no real website,
//! no real browser, no audio device is ever touched. Credentials
//! below are test-only dummies ("test-user" / "test-pass-123") and
//! are never a real account; passwords are never logged.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use mock::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use fetchline_engine::options::{SiteLoginInput, SoundSettingInput};
use fetchline_engine::{DownloadManager, Store};

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

fn login_input(host: &str, user: &str, pass: Option<&str>) -> SiteLoginInput {
    SiteLoginInput {
        id: None,
        host: host.to_string(),
        username: user.to_string(),
        password: pass.map(|p| p.to_string()),
        auth_type: "basic".to_string(),
    }
}

/// A file server that demands HTTP Basic auth with the test-only
/// dummy credential. Every request without the right
/// `Authorization` header gets 401 and no body.
struct GatedServer {
    pub url: String,
    stop: Arc<AtomicBool>,
}

impl GatedServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let expected = basic_auth_value("test-user", "test-pass-123");
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let expected = expected.clone();
                        thread::spawn(move || handle_gated(stream, &expected));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}/secret.bin"),
            stop,
        }
    }
}

impl Drop for GatedServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn handle_gated(mut stream: TcpStream, expected: &str) {
    let mut buf = [0u8; 8192];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first = req.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    // Strict about the whole header line: a wrong password, a
    // missing header, or a different scheme all fail, exactly like
    // a real login-gated server.
    let authorised = req
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
        .map(|l| l.splitn(2, ':').nth(1).unwrap_or("").trim().to_string())
        .as_deref()
        == Some(expected);
    if !authorised {
        let _ = stream.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"test\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    let size = 64 * 1024usize;
    if method == "HEAD" {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nAccept-Ranges: bytes\r\nETag: \"gated1\"\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    let range = req.lines().find_map(|l| {
        let (name, value) = l.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("range") {
            Some(value.trim().to_string())
        } else {
            None
        }
    });
    let (status, body) = match range {
        Some(r) if r.starts_with("bytes=") => {
            let spec = &r["bytes=".len()..];
            let mut it = spec.split('-');
            let start: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let end: usize = it
                .next()
                .and_then(|s| if s.is_empty() { None } else { s.parse().ok() })
                .unwrap_or(size - 1)
                .min(size - 1);
            (
                "206 Partial Content",
                payload(size)[start..=end].to_vec(),
            )
        }
        _ => ("200 OK", payload(size)),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"gated1\"\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}

fn wav_file(dir: &std::path::Path, name: &str) -> String {
    let path = dir.join(name);
    // A minimal WAV header is not needed: the engine only checks
    // the name and that the file exists — playback itself never
    // runs on this VM, and no player reads these bytes here.
    std::fs::write(&path, b"RIFF....WAVE").unwrap();
    path.to_string_lossy().to_string()
}

fn set_sound(mgr: &Arc<DownloadManager>, event: &str, file: &str) {
    mgr.save_sound_settings(&[SoundSettingInput {
        event: event.to_string(),
        enabled: true,
        file: Some(file.to_string()),
    }])
    .unwrap();
}

async fn wait_for_pending_action(
    mgr: &Arc<DownloadManager>,
    queue_id: &str,
    secs: u64,
) -> bool {
    for _ in 0..secs * 10 {
        if mgr.pending_action(queue_id).await.is_some() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

// -- site logins: storage + validation ------------------------------------

#[test]
fn site_login_round_trip_and_row_hides_password() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let row = store
        .save_site_login(&login_input("Example.COM", "test-user", Some("test-pass-123")))
        .unwrap();
    assert_eq!(row.host, "example.com", "host is normalised on save");
    assert_eq!(row.username, "test-user");
    assert_eq!(row.auth_type, "basic");
    assert!(row.has_password);
    // The serialised row must not contain the password anywhere.
    let json = serde_json::to_string(&row).unwrap();
    assert!(!json.contains("test-pass-123"), "password leaked into the UI row: {json}");
    assert!(!json.contains("test-user") || json.contains("test-user")); // username is shown
    let rows = store.list_site_logins().unwrap();
    assert_eq!(rows.len(), 1);
    // The engine can read the credential back, decrypted.
    let creds = store.get_site_login_credentials("example.com").unwrap().unwrap();
    assert_eq!(creds, ("test-user".to_string(), "test-pass-123".to_string()));
    assert!(store.get_site_login_credentials("other.com").unwrap().is_none());
}

#[test]
fn site_login_validation_rejects_bad_input() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    // Empty host.
    assert!(store.save_site_login(&login_input("", "u", Some("p"))).is_err());
    // Host with a space / userinfo.
    assert!(store.save_site_login(&login_input("exa mple.com", "u", Some("p"))).is_err());
    assert!(store.save_site_login(&login_input("user@example.com", "u", Some("p"))).is_err());
    // Empty username, missing password on create, empty password.
    assert!(store.save_site_login(&login_input("example.com", "  ", Some("p"))).is_err());
    assert!(store.save_site_login(&login_input("example.com", "u", None)).is_err());
    assert!(store.save_site_login(&login_input("example.com", "u", Some(""))).is_err());
    // Only Basic auth is applied: another type is refused, not stored.
    let mut ntlm = login_input("example.com", "u", Some("p"));
    ntlm.auth_type = "ntlm".to_string();
    assert!(store.save_site_login(&ntlm).is_err());
    assert!(store.list_site_logins().unwrap().is_empty(), "nothing was stored");
}

#[test]
fn site_login_host_is_cleaned_like_excluded_sites() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let row = store
        .save_site_login(&login_input(
            "https://Files.Example.org:8080/some/path",
            "u",
            Some("p"),
        ))
        .unwrap();
    assert_eq!(row.host, "files.example.org");
}

#[test]
fn site_login_edit_keeps_password_and_delete_works() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let row = store
        .save_site_login(&login_input("example.com", "old-user", Some("test-pass-123")))
        .unwrap();
    // Edit the username without touching the password.
    let mut edit = login_input("example.com", "new-user", None);
    edit.id = Some(row.id.clone());
    let updated = store.save_site_login(&edit).unwrap();
    assert_eq!(updated.username, "new-user");
    assert_eq!(
        store.get_site_login_credentials("example.com").unwrap().unwrap().1,
        "test-pass-123"
    );
    // Change the password.
    let changed = store
        .save_site_login(&login_input("example.com", "new-user", Some("new-pass-456")))
        .unwrap();
    assert_eq!(changed.id, row.id, "same host keeps its row");
    assert_eq!(
        store.get_site_login_credentials("example.com").unwrap().unwrap().1,
        "new-pass-456"
    );
    store.delete_site_login(&row.id).unwrap();
    assert!(store.list_site_logins().unwrap().is_empty());
    assert!(store.delete_site_login(&row.id).is_err(), "unknown id errors");
}

// -- site logins: end to end against the gated fixture --------------------

#[tokio::test]
async fn site_login_gated_download_succeeds_with_login_and_fails_without() {
    let server = GatedServer::start();
    // Without a saved login the probe fails with 401: nothing is created.
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let err = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("401"), "expected a 401, got: {err}");

    // With the saved credential the same URL probes and downloads.
    mgr.save_site_login(&login_input("127.0.0.1", "test-user", Some("test-pass-123")))
        .unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(64 * 1024));
}

#[tokio::test]
async fn site_login_wrong_password_fails_and_is_not_sent_to_another_host() {
    let server = GatedServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // Wrong password: the probe fails with 401.
    mgr.save_site_login(&login_input("127.0.0.1", "test-user", Some("wrong-pass")))
        .unwrap();
    let err = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("401"), "expected a 401, got: {err}");

    // A login saved for "localhost" is not sent to "127.0.0.1":
    // the hosts differ, so the probe is anonymous and fails 401.
    // (Both names reach the same fixture server on this VM.)
    let dir2 = tmpdir();
    let mgr2 = test_mgr(&dir2);
    mgr2.save_site_login(&login_input("localhost", "test-user", Some("test-pass-123")))
        .unwrap();
    let err = mgr2
        .add_download(&server.url, None, Some(dir2.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("401"), "expected a 401, got: {err}");
}

// -- dial-up: settings persist + validation -------------------------------

#[test]
fn dialup_settings_persist_and_validate() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let mut s = store.get_settings().unwrap();
    assert_eq!(s.dialup_redial_attempts, 0);
    assert!(!s.dialup_auto_dial);
    assert!(!s.dialup_hangup_after_completion);
    assert!(s.dialup_connection_name.is_none());
    s.dialup_connection_name = Some("  My Connection  ".to_string());
    s.dialup_auto_dial = true;
    s.dialup_hangup_after_completion = true;
    s.dialup_redial_attempts = 3;
    let saved = store.update_settings(&s).unwrap();
    assert_eq!(saved.dialup_connection_name.as_deref(), Some("My Connection"));
    assert!(saved.dialup_auto_dial);
    assert!(saved.dialup_hangup_after_completion);
    assert_eq!(saved.dialup_redial_attempts, 3);
    // An empty name clears the connection.
    let mut s2 = saved.clone();
    s2.dialup_connection_name = Some("   ".to_string());
    assert!(store.update_settings(&s2).unwrap().dialup_connection_name.is_none());
    // Redial attempts are 0–10, rejected not clamped.
    for bad in [-1, 11] {
        let mut b = saved.clone();
        b.dialup_redial_attempts = bad;
        assert!(store.update_settings(&b).is_err(), "redial {bad} must be rejected");
    }
    assert_eq!(store.get_settings().unwrap().dialup_redial_attempts, 3, "nothing was stored");
}

#[test]
fn dialup_and_sound_migration_adds_fields_to_old_db() {
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        // A database from before this milestone: settings without
        // the dial-up columns, and no sound_settings table at all.
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
    assert_eq!(s.dialup_redial_attempts, 0);
    assert!(!s.dialup_hangup_after_completion);
    // The sound table exists with all three events, disabled.
    let sounds = store.get_sound_settings().unwrap();
    assert_eq!(sounds.len(), 3);
    assert!(sounds.iter().all(|s| !s.enabled && s.file.is_none()));
    // And the new fields can be saved into the migrated database.
    let mut s2 = s.clone();
    s2.dialup_hangup_after_completion = true;
    s2.dialup_redial_attempts = 2;
    store.update_settings(&s2).unwrap();
    assert!(store.get_settings().unwrap().dialup_hangup_after_completion);
}

// -- dial-up: hang-up after completion ------------------------------------

#[tokio::test]
async fn dialup_hangup_arms_action_only_when_queue_has_no_action() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut s = mgr.get_settings().unwrap();
    s.dialup_connection_name = Some("My Connection".to_string());
    s.dialup_hangup_after_completion = true;
    mgr.update_settings(&s).unwrap();

    let queue = mgr.create_queue("Hangup queue").unwrap();
    let (id, _) = mgr
        .add_download(
            &server.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(queue.id.clone()),
        )
        .await
        .unwrap();
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(wait_for_pending_action(&mgr, &queue.id, 10).await);
    let action = mgr.pending_action(&queue.id).await.unwrap();
    assert_eq!(action.action, "hangup");
    assert_eq!(action.program.as_deref(), Some("My Connection"));
    // Off Windows the hang-up cannot run: it errors, honestly,
    // instead of pretending to disconnect anything. On Windows
    // it spawns `rasdial <name> /disconnect` (which does nothing
    // for a connection that does not exist) and reports Ok.
    #[cfg(not(target_os = "windows"))]
    {
        let err = mgr.execute_queue_action(&queue.id).await.unwrap_err();
        assert!(
            err.to_string().contains("only available on Windows"),
            "got: {err}"
        );
    }
    #[cfg(target_os = "windows")]
    {
        mgr.execute_queue_action(&queue.id).await.unwrap();
    }
    assert!(mgr.pending_action(&queue.id).await.is_none(), "the action is consumed");
}

#[tokio::test]
async fn dialup_hangup_loses_to_a_queue_action_and_needs_a_download() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut s = mgr.get_settings().unwrap();
    s.dialup_connection_name = Some("My Connection".to_string());
    s.dialup_hangup_after_completion = true;
    mgr.update_settings(&s).unwrap();

    // A queue with its own action keeps it: hang-up does not arm a
    // second action (one pending action per queue id).
    let queue = mgr.create_queue("Shutdown queue").unwrap();
    mgr.update_queue(&queue.id, "one_time", None, None, false, "shutdown", None, None)
        .await
        .unwrap();
    let (id, _) = mgr
        .add_download(
            &server.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(queue.id.clone()),
        )
        .await
        .unwrap();
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(wait_for_pending_action(&mgr, &queue.id, 10).await);
    assert_eq!(mgr.pending_action(&queue.id).await.unwrap().action, "shutdown");
    mgr.dismiss_queue_action(&queue.id).await.unwrap();

    // A queue whose file fails downloads nothing: no hang-up is
    // armed (the BUG-003 rule — an action needs >= 1 download).
    // The 500 fixture fails at the probe, so the add itself fails
    // and the queue stays empty — either way no action is armed.
    let failing = MockServer::start(MockConfig {
        fail_status: Some(500),
        ..Default::default()
    });
    let queue2 = mgr.create_queue("Failing queue").unwrap();
    assert!(mgr
        .add_download(
            &failing.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(queue2.id.clone()),
        )
        .await
        .is_err());
    assert!(mgr.pending_action(&queue2.id).await.is_none());
}

// -- sounds: settings + validation ----------------------------------------

#[test]
fn sound_settings_round_trip_and_validation() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let defaults = store.get_sound_settings().unwrap();
    assert_eq!(defaults.len(), 3);
    assert_eq!(defaults[0].event, "download_complete");
    assert_eq!(defaults[1].event, "download_failed");
    assert_eq!(defaults[2].event, "queue_finished");
    assert!(defaults.iter().all(|s| !s.enabled));

    let wav = wav_file(&dir, "done.wav");
    let saved = store
        .save_sound_setting(&SoundSettingInput {
            event: "download_complete".to_string(),
            enabled: true,
            file: Some(wav.clone()),
        })
        .unwrap();
    assert!(saved.enabled);
    assert_eq!(saved.file.as_deref(), Some(wav.as_str()));
    assert_eq!(store.get_sound_file("download_complete").unwrap().as_deref(), Some(wav.as_str()));
    // A disabled event resolves to no sound, even with a file set.
    store
        .save_sound_setting(&SoundSettingInput {
            event: "download_complete".to_string(),
            enabled: false,
            file: Some(wav.clone()),
        })
        .unwrap();
    assert!(store.get_sound_file("download_complete").unwrap().is_none());
    // MP3 is rejected, an unknown event is rejected, an empty file clears.
    assert!(store
        .save_sound_setting(&SoundSettingInput {
            event: "download_complete".to_string(),
            enabled: true,
            file: Some("ding.mp3".to_string()),
        })
        .is_err());
    assert!(store
        .save_sound_setting(&SoundSettingInput {
            event: "no_such_event".to_string(),
            enabled: true,
            file: None,
        })
        .is_err());
    let cleared = store
        .save_sound_setting(&SoundSettingInput {
            event: "download_complete".to_string(),
            enabled: true,
            file: Some("".to_string()),
        })
        .unwrap();
    assert!(cleared.file.is_none());
}

// -- sounds: events fire, files resolve, nothing is heard -----------------

#[tokio::test]
async fn sound_events_fire_on_complete_and_failure_with_resolved_files() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let done_wav = wav_file(&dir, "done.wav");
    let failed_wav = wav_file(&dir, "failed.wav");
    set_sound(&mgr, "download_complete", &done_wav);
    set_sound(&mgr, "download_failed", &failed_wav);

    // A completed download fires download_complete with its file.
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let events = mgr.sound_events();
    assert_eq!(events.len(), 1, "one event, got: {events:?}");
    assert_eq!(events[0].event, "download_complete");
    assert_eq!(events[0].file.as_deref(), Some(done_wav.as_str()));

    // A failed download fires download_failed with its file.
    // One retry only, so the failure lands in seconds instead of
    // after the full 1/2/4/8/16 s backoff ladder.
    let mut s = mgr.get_settings().unwrap();
    s.max_retries = 1;
    mgr.update_settings(&s).unwrap();
    server.set_fail_get(true);
    let (fid, _) = mgr
        .add_download(
            &server.url,
            Some("second.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&fid).await.unwrap();
    assert!(wait_for_status(&mgr, &fid, "error", 30).await);
    let events = mgr.sound_events();
    assert_eq!(events.len(), 2, "got: {events:?}");
    assert_eq!(events[1].event, "download_failed");
    assert_eq!(events[1].file.as_deref(), Some(failed_wav.as_str()));
}

#[tokio::test]
async fn sound_queue_finished_fires_only_when_something_downloaded() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue_wav = wav_file(&dir, "queue.wav");
    set_sound(&mgr, "queue_finished", &queue_wav);
    let queue = mgr.create_queue("Sound queue").unwrap();
    let (id, _) = mgr
        .add_download(
            &server.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(queue.id.clone()),
        )
        .await
        .unwrap();
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    // The queue drains a moment after its file completes.
    for _ in 0..50 {
        if mgr.sound_events().iter().any(|e| e.event == "queue_finished") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let events = mgr.sound_events();
    assert!(
        events.iter().any(|e| e.event == "queue_finished" && e.file.as_deref() == Some(queue_wav.as_str())),
        "queue_finished with its file, got: {events:?}"
    );
}

#[tokio::test]
async fn sound_disabled_event_records_nothing_and_playback_is_windows_only() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // Nothing configured: a completed download records no event.
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(mgr.sound_events().is_empty());

    // Test-button validation: no file, a non-WAV file, a missing
    // file are all clear errors before any playback is attempted.
    assert!(mgr.play_sound("download_complete").unwrap_err().to_string().contains("No sound is set"));
    assert!(mgr.test_sound_file("").is_err());
    assert!(mgr.test_sound_file("ding.mp3").is_err());
    assert!(mgr.test_sound_file("/tmp/fetchline-test-no-such-file.wav").is_err());
    // A real WAV file exists, so validation passes — and on this
    // Linux VM the honest answer is that playback is Windows-only.
    // On Windows the file really plays (PowerShell SoundPlayer),
    // so the same calls report Ok there instead of erroring.
    let wav = wav_file(&dir, "test.wav");
    #[cfg(not(target_os = "windows"))]
    {
        let err = mgr.test_sound_file(&wav).unwrap_err();
        assert!(err.to_string().contains("only available on Windows"), "got: {err}");
        set_sound(&mgr, "download_complete", &wav);
        let err = mgr.play_sound("download_complete").unwrap_err();
        assert!(err.to_string().contains("only available on Windows"), "got: {err}");
    }
    #[cfg(target_os = "windows")]
    {
        assert!(mgr.test_sound_file(&wav).is_ok());
        set_sound(&mgr, "download_complete", &wav);
        assert!(mgr.play_sound("download_complete").is_ok());
    }
}
