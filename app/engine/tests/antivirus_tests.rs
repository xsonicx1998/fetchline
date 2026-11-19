//! Antivirus hook (S14) tests.
//!
//! Every test uses a FAKE scanner script fixture (a tiny shell
//! script that exits 0 / non-zero / sleeps) — no real antivirus
//! product is ever run, and no malware sample (not even an EICAR
//! test file) is ever fetched, created or used. Downloads come
//! from the shared local mock HTTP server only.

#![allow(dead_code)]

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fetchline_engine::{run_scan, DownloadManager, ScanResult, Store};

#[path = "common/mock.rs"]
mod mock;

use mock::{payload, tmpdir, wait_for_status, MockConfig, MockServer};

fn test_mgr(dir: &Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

/// Point the manager at a fake scanner: `command` is the full
/// template (with `{file}`), `enabled` the on/off switch,
/// `timeout_secs` the scan timeout.
fn set_scanner(mgr: &DownloadManager, command: Option<&str>, enabled: bool, timeout_secs: i64) {
    let mut settings = mgr.get_settings().unwrap();
    settings.antivirus_command = command.map(str::to_string);
    settings.antivirus_enabled = enabled;
    settings.antivirus_timeout_secs = timeout_secs;
    mgr.update_settings(&settings).unwrap();
}

/// Write a fake scanner script into `dir` and make it executable.
/// `body` is the script body after the shebang line.
/// On Windows the same body is translated to a .bat file (the
/// engine runs the scanner command directly, and CreateProcess
/// handles .bat files); the bodies used here are a closed set
/// (`exit N`, `sleep N`, `echo ...`, and the `printf ... >> <record>`
/// recorder), and any other line fails loudly instead of silently
/// passing. The returned path is the script to put in the command
/// template — `.sh` on Unix, `.bat` on Windows.
fn fake_scanner(dir: &Path, name: &str, body: &str) -> PathBuf {
    #[cfg(windows)]
    let path = dir.join(format!("{}.bat", name.strip_suffix(".sh").unwrap_or(name)));
    #[cfg(not(windows))]
    let path = dir.join(name);
    #[cfg(windows)]
    std::fs::write(
        &path,
        format!("@echo off\n{}\n", translate_body_to_batch(body)),
    )
    .unwrap();
    #[cfg(not(windows))]
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Translate a fake-scanner shell body to batch, line by line.
/// Only the constructs the tests actually use are supported.
#[cfg(windows)]
fn translate_body_to_batch(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(record) = line.strip_prefix("printf '%s\\n' \"$1\" >> ") {
            // Recorder: write the scanned path into the record
            // file. `%~1` (not `%1`): Rust quotes the argument
            // when it invokes a .bat, and `%1` keeps those
            // quotes while `%~1` strips them — the recorded
            // path must be the bare path, as `$1` is on Unix
            // (BUG-035: the record held `"C:\…\file.bin"`).
            out.push_str(&format!("@echo %~1>> \"{}\"\n", record.trim()));
        } else if let Some(secs) = line.strip_prefix("sleep ") {
            // `timeout` refuses redirected stdin (the engine runs
            // the scanner with stdin nulled); ping is the classic
            // batch sleep and works with stdin redirected.
            let n: u64 = secs.trim().parse().expect("sleep N must be a number");
            out.push_str(&format!("ping -n {} 127.0.0.1 >nul\n", n + 1));
        } else if let Some(code) = line.strip_prefix("exit ") {
            out.push_str(&format!("exit /b {}\n", code.trim()));
        } else if let Some(rest) = line.strip_prefix("echo ") {
            // `%~1` strips the quotes Rust adds when invoking
            // a .bat (see the recorder above, BUG-035).
            let rest = rest.replace("$1", "%~1");
            let rest = rest
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .unwrap_or(&rest);
            out.push_str(&format!("echo {rest}\n"));
        } else {
            panic!("fake_scanner: no Windows translation for shell line: {line}");
        }
    }
    out
}

async fn complete_download(
    mgr: &Arc<DownloadManager>,
    server_url: &str,
    dir: &Path,
    name: &str,
) -> String {
    let (id, _) = mgr
        .add_download(
            server_url,
            Some(name.to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(mgr, &id, "complete", 30).await);
    id
}

/// Poll the stored scan result until it is a finished outcome
/// (anything but `scanning`), or `None` after `secs` seconds.
async fn wait_for_scan(mgr: &DownloadManager, id: &str, secs: u64) -> Option<ScanResult> {
    for _ in 0..secs * 10 {
        if let Ok(Some(r)) = mgr.get_scan_result(id) {
            if r.status != "scanning" {
                return Some(r);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    mgr.get_scan_result(id).ok().flatten()
}

#[tokio::test]
async fn clean_scanner_reports_clean_and_scans_the_final_path() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    // The fake scanner records the path it was given, so the test
    // can prove the scan ran on the finished file, not the .part.
    let record = dir.join("scanned-paths.txt");
    let script = fake_scanner(
        &dir,
        "clean.sh",
        &format!("printf '%s\\n' \"$1\" >> {}", record.display()),
    );
    set_scanner(
        &mgr,
        Some(&format!("{} {{file}}", script.display())),
        true,
        60,
    );

    let id = complete_download(&mgr, &server.url, &dir, "clean.bin").await;
    let result = wait_for_scan(&mgr, &id, 10)
        .await
        .expect("a scan result must be recorded");
    assert_eq!(result.status, "clean", "{result:?}");
    assert!(result.scanned_at.is_some());

    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.scan_status.as_deref(), Some("clean"));
    // The scanned path is the final file — never the .part — and
    // the file on disk is byte-exact.
    let scanned = std::fs::read_to_string(&record).unwrap();
    assert_eq!(scanned.trim(), row.save_path, "scanner saw {scanned:?}");
    assert!(!row.save_path.ends_with(".part"));
    assert!(!Path::new(&format!("{}.part", row.save_path)).exists());
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    // A clean file may be opened (xdg-open on this VM: spawn only).
    assert!(mgr.open_file(&id).is_ok(), "a clean file must open");
}

#[tokio::test]
async fn threat_scanner_reports_infected_and_nothing_is_auto_deleted_or_opened() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    let script = fake_scanner(
        &dir,
        "threat.sh",
        "echo \"threat found: FakeTest.Threat in $1\"\nexit 1",
    );
    set_scanner(
        &mgr,
        Some(&format!("{} {{file}}", script.display())),
        true,
        60,
    );

    let id = complete_download(&mgr, &server.url, &dir, "threat.bin").await;
    let result = wait_for_scan(&mgr, &id, 10)
        .await
        .expect("a scan result must be recorded");
    assert_eq!(result.status, "infected", "{result:?}");
    assert!(
        result
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("FakeTest.Threat"),
        "the detail must carry the scanner's own report: {result:?}"
    );

    // Nothing is auto-deleted or auto-quarantined: the file is
    // still exactly where the download put it.
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(Path::new(&row.save_path).is_file());
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    // Open is refused for a flagged file — never auto-opened.
    let err = mgr.open_file(&id).unwrap_err();
    assert!(err.to_string().contains("threat"), "{err}");
    // Quarantine is an explicit user choice: it moves the file
    // into a Quarantine folder and points the row at it.
    let new_path = mgr.quarantine_download(&id).unwrap();
    assert!(new_path.contains("Quarantine"), "{new_path}");
    assert!(Path::new(&new_path).is_file());
    assert!(!Path::new(&row.save_path).exists());
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.save_path, new_path);
    assert_eq!(row.scan_status.as_deref(), Some("infected"));
}

#[tokio::test]
async fn missing_scanner_command_is_a_clear_scan_failure() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    set_scanner(&mgr, Some("/nonexistent/no-such-scanner {file}"), true, 60);

    let id = complete_download(&mgr, &server.url, &dir, "missing.bin").await;
    let result = wait_for_scan(&mgr, &id, 10)
        .await
        .expect("a scan result must be recorded");
    assert_eq!(result.status, "scan_failed", "{result:?}");
    assert!(
        result
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("Could not start"),
        "{result:?}"
    );
    // The download itself still completed — a broken scanner
    // never fails the download.
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().status, "complete");
}

#[tokio::test]
async fn no_scanner_configured_completes_normally_with_not_configured() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    // Default settings: no antivirus command at all.
    assert!(mgr.get_settings().unwrap().antivirus_command.is_none());

    let id = complete_download(&mgr, &server.url, &dir, "plain.bin").await;
    let result = mgr
        .get_scan_result(&id)
        .unwrap()
        .expect("not-configured must be recorded at once");
    assert_eq!(result.status, "not_configured", "{result:?}");
    assert!(result
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("No antivirus scanner is configured"));
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.scan_status.as_deref(), Some("not_configured"));
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn scanning_turned_off_records_not_configured_and_never_runs_the_scanner() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    let record = dir.join("scanned-paths.txt");
    let script = fake_scanner(
        &dir,
        "clean.sh",
        &format!("printf '%s\\n' \"$1\" >> {}", record.display()),
    );
    set_scanner(
        &mgr,
        Some(&format!("{} {{file}}", script.display())),
        false,
        60,
    );

    let id = complete_download(&mgr, &server.url, &dir, "off.bin").await;
    let result = mgr
        .get_scan_result(&id)
        .unwrap()
        .expect("not-configured must be recorded at once");
    assert_eq!(result.status, "not_configured", "{result:?}");
    assert!(result
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("turned off"));
    // The scanner was never executed.
    assert!(!record.exists());
    // A manual re-scan while off reports the same, without running.
    let again = mgr.run_antivirus_scan(&id).await.unwrap();
    assert_eq!(again.status, "not_configured");
    assert!(!record.exists());
}

#[tokio::test]
async fn scan_does_not_block_completion_and_a_slow_scanner_times_out() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    let script = fake_scanner(&dir, "slow.sh", "sleep 30\nexit 0");
    set_scanner(
        &mgr,
        Some(&format!("{} {{file}}", script.display())),
        true,
        5,
    );

    let (id, _) = mgr
        .add_download(
            &server.url,
            Some("slow.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    // The download completes while the scanner is still asleep:
    // completion never waits for the scan.
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.scan_status.as_deref(), Some("scanning"));
    // The app stays responsive while the scan runs.
    assert!(mgr.list_downloads().is_ok());
    assert!(mgr.get_scan_result(&id).is_ok());
    // After the 5 s timeout the scan is killed and recorded —
    // the test takes ~5 s, not 30 s, which proves the kill.
    let result = wait_for_scan(&mgr, &id, 15)
        .await
        .expect("a scan result must be recorded");
    assert_eq!(result.status, "timed_out", "{result:?}");
    assert!(result
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("did not finish within 5 seconds"));
}

#[tokio::test]
async fn run_scan_timeout_kills_a_sleeping_scanner_quickly() {
    let dir = tmpdir();
    let script = fake_scanner(&dir, "sleep.sh", "sleep 30\nexit 0");
    let target = dir.join("target.bin");
    std::fs::write(&target, b"hello").unwrap();
    let start = std::time::Instant::now();
    let result = run_scan(&format!("{} {{file}}", script.display()), &target, 1).await;
    assert_eq!(result.status, "timed_out", "{result:?}");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "the scan must be killed at the timeout, not after the 30 s sleep (took {:?})",
        start.elapsed()
    );
}

#[tokio::test]
async fn exit_zero_with_a_threat_in_the_output_is_still_infected() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    // Some scanners exit 0 and report the threat in text
    // (ClamAV-style `path: Name FOUND`).
    let script = fake_scanner(
        &dir,
        "found.sh",
        "echo \"$1: FakeTest.Threat FOUND\"\nexit 0",
    );
    set_scanner(
        &mgr,
        Some(&format!("{} {{file}}", script.display())),
        true,
        60,
    );

    let id = complete_download(&mgr, &server.url, &dir, "found.bin").await;
    let result = wait_for_scan(&mgr, &id, 10)
        .await
        .expect("a scan result must be recorded");
    assert_eq!(result.status, "infected", "{result:?}");
    assert!(result
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("FakeTest.Threat"));
}

#[tokio::test]
async fn manual_rescan_after_completion_records_a_new_result() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    // Complete with no scanner: not_configured.
    let id = complete_download(&mgr, &server.url, &dir, "rescan.bin").await;
    assert_eq!(
        mgr.get_scan_result(&id).unwrap().unwrap().status,
        "not_configured"
    );
    // Configure a clean scanner and re-scan by hand.
    let script = fake_scanner(&dir, "clean.sh", "exit 0");
    set_scanner(
        &mgr,
        Some(&format!("{} {{file}}", script.display())),
        true,
        60,
    );
    let result = mgr.run_antivirus_scan(&id).await.unwrap();
    assert_eq!(result.status, "clean", "{result:?}");
    assert_eq!(mgr.get_scan_result(&id).unwrap().unwrap().status, "clean");
}

#[tokio::test]
async fn scan_is_refused_for_an_unfinished_download() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    let (id, _) = mgr
        .add_download(
            &server.url,
            Some("unfinished.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    let err = mgr.run_antivirus_scan(&id).await.unwrap_err();
    assert!(
        err.to_string().contains("Only a finished download"),
        "{err}"
    );
}

#[tokio::test]
async fn quarantine_is_refused_for_a_clean_file() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    let script = fake_scanner(&dir, "clean.sh", "exit 0");
    set_scanner(
        &mgr,
        Some(&format!("{} {{file}}", script.display())),
        true,
        60,
    );

    let id = complete_download(&mgr, &server.url, &dir, "clean2.bin").await;
    assert_eq!(wait_for_scan(&mgr, &id, 10).await.unwrap().status, "clean");
    let err = mgr.quarantine_download(&id).unwrap_err();
    assert!(
        err.to_string()
            .contains("Only a file the antivirus scan flagged"),
        "{err}"
    );
}

#[tokio::test]
async fn settings_round_trip_and_test_button_use_the_fake_scanner() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let clean = fake_scanner(&dir, "clean.sh", "echo clean\nexit 0");
    let threat = fake_scanner(
        &dir,
        "threat.sh",
        "echo \"threat found: FakeTest.Threat\"\nexit 1",
    );

    // Round-trip: command + switch + timeout persist.
    set_scanner(
        &mgr,
        Some(&format!("{} {{file}}", clean.display())),
        true,
        120,
    );
    let saved = mgr.get_settings().unwrap();
    assert_eq!(
        saved.antivirus_command.as_deref(),
        Some(format!("{} {{file}}", clean.display()).as_str())
    );
    assert!(saved.antivirus_enabled);
    assert_eq!(saved.antivirus_timeout_secs, 120);

    // Test button logic against the fixtures: clean, threat,
    // missing scanner, and no command at all.
    let result = mgr
        .test_antivirus_scanner(&format!("{} {{file}}", clean.display()), 60)
        .await
        .unwrap();
    assert_eq!(result.status, "clean", "{result:?}");
    let result = mgr
        .test_antivirus_scanner(&format!("{} {{file}}", threat.display()), 60)
        .await
        .unwrap();
    assert_eq!(result.status, "infected", "{result:?}");
    let result = mgr
        .test_antivirus_scanner("/nonexistent/no-such-scanner {file}", 60)
        .await
        .unwrap();
    assert_eq!(result.status, "scan_failed", "{result:?}");
    let err = mgr.test_antivirus_scanner("", 60).await.unwrap_err();
    assert!(err.to_string().contains("Enter a scanner command"), "{err}");
}

#[test]
fn scan_timeout_validation_rejects_absurd_values() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    for bad in [0, 4, 601, 3600] {
        let mut settings = mgr.get_settings().unwrap();
        settings.antivirus_timeout_secs = bad;
        let err = mgr.update_settings(&settings).unwrap_err();
        assert!(
            err.to_string().contains("scan timeout"),
            "timeout {bad} must be rejected: {err}"
        );
    }
    for good in [5, 60, 600] {
        let mut settings = mgr.get_settings().unwrap();
        settings.antivirus_timeout_secs = good;
        mgr.update_settings(&settings).unwrap();
        assert_eq!(mgr.get_settings().unwrap().antivirus_timeout_secs, good);
    }
}

/// BUG-026 regression: `list_queue_files` must select the S14
/// scan columns too — the first build updated `get_download` /
/// `list_downloads` but missed this third `row_to_download`
/// caller, so every queue read failed with
/// `Db(InvalidColumnIndex(20))` and queues/scheduler broke.
#[tokio::test]
async fn queue_files_read_back_with_scan_columns() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    let (id, _) = mgr
        .add_download(
            &server.url,
            Some("queued.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            Some("queue-main".to_string()),
        )
        .await
        .unwrap();
    let files = mgr.list_queue_files("queue-main").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, id);
    assert_eq!(
        files[0].scan_status, None,
        "a queued file was never scanned"
    );
}

#[tokio::test]
async fn completion_event_carries_the_scan_result() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    let mut events = mgr.subscribe();
    let id = complete_download(&mgr, &server.url, &dir, "event.bin").await;
    // Drain events until the completion event for this download.
    let mut found = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(1), events.recv()).await {
            Ok(Ok(event)) if event.id == id && event.status == "complete" => {
                found = Some(event);
                break;
            }
            _ => continue,
        }
    }
    let event = found.expect("a completion event must arrive");
    let scan = event
        .scan
        .expect("the completion event must carry the scan result");
    assert_eq!(scan.status, "not_configured", "{scan:?}");
    // The row agrees with the event.
    assert_eq!(
        mgr.get_download(&id)
            .unwrap()
            .unwrap()
            .scan_status
            .as_deref(),
        Some("not_configured")
    );
}

#[tokio::test]
async fn scan_result_is_persisted_across_a_store_reopen() {
    let dir = tmpdir();
    let server = MockServer::start(MockConfig::default());
    let mgr = test_mgr(&dir);
    let script = fake_scanner(&dir, "clean.sh", "exit 0");
    set_scanner(
        &mgr,
        Some(&format!("{} {{file}}", script.display())),
        true,
        60,
    );
    let id = complete_download(&mgr, &server.url, &dir, "persist.bin").await;
    assert_eq!(wait_for_scan(&mgr, &id, 10).await.unwrap().status, "clean");
    drop(mgr);
    let reopened = Store::open(&dir.join("app.db")).unwrap();
    let result = reopened
        .get_scan_result(&id)
        .unwrap()
        .expect("result must survive reopen");
    assert_eq!(result.status, "clean");
    assert!(result.scanned_at.is_some());
}

#[test]
fn pre_s14_database_migrates_scan_columns_and_settings() {
    use rusqlite::Connection;
    let dir = tmpdir();
    let db_path = dir.join("app.db");
    // A database made before S14: settings without the antivirus
    // switch/timeout, downloads without the scan columns.
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1),
           default_download_folder TEXT NOT NULL DEFAULT '',
           max_connections INTEGER NOT NULL DEFAULT 8,
           speed_limit_kbps INTEGER, language TEXT NOT NULL DEFAULT 'en',
           theme TEXT NOT NULL DEFAULT 'light',
           browser_integration INTEGER NOT NULL DEFAULT 1,
           browser_start_drained INTEGER NOT NULL DEFAULT 1,
           antivirus_command TEXT, file_types_auto TEXT NOT NULL DEFAULT '[]',
           excluded_sites TEXT NOT NULL DEFAULT '[]',
           check_updates_weekly INTEGER NOT NULL DEFAULT 1,
           start_on_startup INTEGER NOT NULL DEFAULT 0,
           default_action TEXT NOT NULL DEFAULT 'start_now',
           duplicate_action TEXT NOT NULL DEFAULT 'rename',
           confirm_delete INTEGER NOT NULL DEFAULT 1,
           connection_timeout_secs INTEGER NOT NULL DEFAULT 30,
           max_retries INTEGER NOT NULL DEFAULT 5,
           dialup_connection_name TEXT,
           dialup_auto_dial INTEGER NOT NULL DEFAULT 0,
           dialup_hangup_after_completion INTEGER NOT NULL DEFAULT 0,
           dialup_redial_attempts INTEGER NOT NULL DEFAULT 0,
           update_url TEXT NOT NULL DEFAULT 'https://updates.fetchline.app/releases/latest.json',
           last_update_check_at TEXT,
           quota_enabled INTEGER NOT NULL DEFAULT 0,
           quota_mb INTEGER, quota_hours INTEGER,
           updated_at TEXT NOT NULL DEFAULT (datetime('now')));
         INSERT INTO settings (id) VALUES (1);
         CREATE TABLE downloads (id TEXT PRIMARY KEY, url TEXT NOT NULL,
           final_url TEXT, filename TEXT NOT NULL, save_path TEXT NOT NULL,
           category_id TEXT NOT NULL, queue_id TEXT, grabber_project_id TEXT,
           queue_position INTEGER NOT NULL DEFAULT 0,
           size_bytes INTEGER, downloaded_bytes INTEGER NOT NULL DEFAULT 0,
           status TEXT NOT NULL DEFAULT 'queued', speed_bps INTEGER NOT NULL DEFAULT 0,
           eta_seconds INTEGER, speed_limit_kbps INTEGER,
           quota_paused INTEGER NOT NULL DEFAULT 0,
           referrer TEXT, headers_json TEXT NOT NULL DEFAULT '{}',
           cookies_json TEXT NOT NULL DEFAULT '{}', etag TEXT, last_modified TEXT,
           error_message TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')),
           updated_at TEXT NOT NULL DEFAULT (datetime('now')), completed_at TEXT);
         INSERT INTO downloads (id, url, filename, save_path, category_id, status)
           VALUES ('dl-old', 'http://127.0.0.1/file.bin', 'old.bin', '/tmp/old.bin', 'cat-video', 'complete');",
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&db_path).unwrap();
    let settings = store.get_settings().unwrap();
    assert!(
        settings.antivirus_enabled,
        "migration must default scanning ON"
    );
    assert_eq!(settings.antivirus_timeout_secs, 60);
    let row = store
        .get_download("dl-old")
        .unwrap()
        .expect("the old row must survive");
    assert_eq!(row.scan_status, None, "an old row was never scanned");
    // The migrated columns are writable.
    store
        .set_scan_result(
            "dl-old",
            &ScanResult {
                status: "clean".to_string(),
                detail: None,
                scanned_at: Some("2026-11-15T00:00:00Z".to_string()),
            },
        )
        .unwrap();
    assert_eq!(
        store.get_scan_result("dl-old").unwrap().unwrap().status,
        "clean"
    );
}
