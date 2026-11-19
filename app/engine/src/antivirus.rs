//! Antivirus hook (S14, "Automatic antivirus scan on completion").
//!
//! Hook only — Fetchline does NOT include an antivirus engine, does
//! NOT bundle any scanner and never downloads one. On download
//! completion the engine runs a scanner command the user configured
//! in Settings (`settings.antivirus_command`, e.g. Windows Defender's
//! `MpCmdRun.exe -Scan -ScanType 3 -File "{file}"`) against the
//! finished file, and records the result on the download row. The
//! scanner itself is the user's own installed product; Fetchline
//! only invokes it and reports what it said.
//!
//! The command template substitutes `{file}` with the finished
//! file's path. When the template has no `{file}`, the path is
//! appended as the last argument. The template is split with the
//! small tokenizer below (quotes honoured) and executed directly —
//! never through a shell — so a filename cannot inject a second
//! command. A scanner that needs a shell pipeline cannot be
//! expressed here; that is deliberate.
//!
//! Outcomes (`ScanResult.status`):
//! - `clean` — the scanner exited 0 and reported nothing threatening.
//! - `infected` — the scanner exited non-zero, or exited 0 while its
//!   output names a threat (`FOUND`, `infected`, `threat found`,
//!   `virus found`). The detail is the scanner's own first line(s),
//!   truncated — Fetchline never invents a threat name.
//! - `scan_failed` — the scanner could not be started (missing
//!   program, bad template) or the file is gone. A clear error, not
//!   a silent pass.
//! - `not_configured` — no scanner is configured, or scanning is
//!   turned off in Settings. The download still completes normally.
//! - `timed_out` — the scanner ran longer than
//!   `settings.antivirus_timeout_secs` (default 60 s, 5–600 s). It
//!   is killed; the app never hangs waiting for it.
//! - `scanning` — a scan is running right now (transient; stored
//!   while the detached completion scan runs, so the dialog can
//!   show "Scanning…" instead of stale data).
//!
//! Nothing is ever auto-deleted or auto-quarantined: a threat only
//! disables "Open" in the completion dialog and offers Delete /
//! Quarantine as explicit user choices (see `DownloadManager::
//! quarantine_download` and `open_file`). The scan runs after the
//! `.part` file has been renamed to its final name, so the scanner
//! always sees the finished file, never a partial one, and it runs
//! on a detached task — a slow scanner never blocks the completion
//! path, the queue drain, or the UI.
//!
//! Tests use a FAKE scanner script fixture only (exit codes and
//! output simulated). No real antivirus product is ever run here,
//! and no malware sample — not even an EICAR test file — is ever
//! fetched, created or used.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task;

use crate::error::{EngineError, Result};
use crate::timeutil::{now_unix, unix_to_iso};

/// The `{file}` placeholder in a scanner command template.
pub const FILE_PLACEHOLDER: &str = "{file}";

/// Default scan timeout (seconds) when no setting is stored.
pub const DEFAULT_SCAN_TIMEOUT_SECS: i64 = 60;
/// Smallest / largest scan timeout a setting may hold.
pub const MIN_SCAN_TIMEOUT_SECS: i64 = 5;
pub const MAX_SCAN_TIMEOUT_SECS: i64 = 600;

/// How much scanner output is kept for the detail (bytes, before
/// the detail itself is truncated to a readable length).
pub const MAX_SCAN_OUTPUT_BYTES: usize = 64 * 1024;
/// Longest `detail` string stored / shown.
pub const MAX_SCAN_DETAIL_CHARS: usize = 500;

pub const SCAN_CLEAN: &str = "clean";
pub const SCAN_INFECTED: &str = "infected";
pub const SCAN_FAILED: &str = "scan_failed";
pub const SCAN_NOT_CONFIGURED: &str = "not_configured";
pub const SCAN_TIMED_OUT: &str = "timed_out";
pub const SCAN_SCANNING: &str = "scanning";

/// Every scan status the dialog can show.
pub const SCAN_STATUSES: [&str; 6] = [
    SCAN_CLEAN,
    SCAN_INFECTED,
    SCAN_FAILED,
    SCAN_NOT_CONFIGURED,
    SCAN_TIMED_OUT,
    SCAN_SCANNING,
];

/// The result of one antivirus scan, as stored on the download row
/// and shown in the completion dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanResult {
    pub status: String,
    /// The scanner's own report (first line(s) of its output for a
    /// threat, or the reason a scan failed / was skipped). `None`
    /// for a plain clean result.
    #[serde(default)]
    pub detail: Option<String>,
    /// When the scan finished, UTC ISO-8601. `None` while scanning.
    #[serde(default)]
    pub scanned_at: Option<String>,
}

impl ScanResult {
    pub fn clean() -> Self {
        Self {
            status: SCAN_CLEAN.to_string(),
            detail: None,
            scanned_at: Some(unix_to_iso(now_unix())),
        }
    }

    pub fn not_configured(detail: &str) -> Self {
        Self {
            status: SCAN_NOT_CONFIGURED.to_string(),
            detail: Some(detail.to_string()),
            scanned_at: Some(unix_to_iso(now_unix())),
        }
    }

    pub fn scanning() -> Self {
        Self {
            status: SCAN_SCANNING.to_string(),
            detail: None,
            scanned_at: None,
        }
    }

    fn failed(detail: String) -> Self {
        Self {
            status: SCAN_FAILED.to_string(),
            detail: Some(truncate_detail(&detail)),
            scanned_at: Some(unix_to_iso(now_unix())),
        }
    }

    fn timed_out(timeout_secs: i64) -> Self {
        Self {
            status: SCAN_TIMED_OUT.to_string(),
            detail: Some(format!(
                "The antivirus scan did not finish within {timeout_secs} seconds, so it was stopped. The file was not scanned — check the scanner or raise the timeout in Settings."
            )),
            scanned_at: Some(unix_to_iso(now_unix())),
        }
    }

    fn infected(detail: String) -> Self {
        Self {
            status: SCAN_INFECTED.to_string(),
            detail: Some(truncate_detail(&detail)),
            scanned_at: Some(unix_to_iso(now_unix())),
        }
    }
}

/// Is `timeout_secs` a timeout the settings may hold? Rejected,
/// not clamped (the BUG-008 rule): a typo must be visible.
pub fn validate_scan_timeout(timeout_secs: i64) -> Result<()> {
    if !(MIN_SCAN_TIMEOUT_SECS..=MAX_SCAN_TIMEOUT_SECS).contains(&timeout_secs) {
        return Err(EngineError::InvalidState(format!(
            "The antivirus scan timeout must be between {MIN_SCAN_TIMEOUT_SECS} and {MAX_SCAN_TIMEOUT_SECS} seconds"
        )));
    }
    Ok(())
}

/// Split a scanner command template into argv, honouring single
/// and double quotes and backslash escapes. An unterminated quote
/// is an error, not a silent truncation.
pub fn split_command(template: &str) -> Result<Vec<String>> {
    let mut args: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_token = false;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                has_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                has_token = true;
            }
            '\\' if !in_single => {
                // A backslash escapes the next char only when
                // that char needs escaping. Inside double quotes
                // that is a quote or another backslash; outside
                // quotes it is a quote, another backslash, or a
                // space. Anything else keeps the backslash —
                // an unquoted Windows path like
                // `C:\Scanner\scan.exe` must keep its
                // backslashes (BUG-034: every one was stripped,
                // so the scanner program could never be found
                // on Windows and every scan failed at spawn).
                match chars.peek() {
                    Some(&next) if in_double && next != '"' && next != '\\' => {
                        cur.push('\\');
                    }
                    Some(&next)
                        if !in_double
                            && next != '"'
                            && next != '\''
                            && next != '\\'
                            && !next.is_whitespace() =>
                    {
                        cur.push('\\');
                    }
                    Some(&next) => {
                        cur.push(next);
                        chars.next();
                    }
                    None => cur.push('\\'),
                }
                has_token = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if has_token {
                    args.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if in_single || in_double {
        return Err(EngineError::InvalidState(
            "The antivirus command has an unterminated quote — check it in Settings".to_string(),
        ));
    }
    if has_token {
        args.push(cur);
    }
    if args.is_empty() {
        return Err(EngineError::InvalidState(
            "No antivirus scanner is configured — enter a scanner command in Settings".to_string(),
        ));
    }
    Ok(args)
}

/// Build the command to run for `file_path`: substitute `{file}`
/// in every argument that carries it; when no argument carries it,
/// append the path as the last argument.
pub fn build_command(template: &str, file_path: &Path) -> Result<std::process::Command> {
    let args = split_command(template)?;
    let path = file_path.to_string_lossy().to_string();
    let mut substituted: Vec<String> = args
        .iter()
        .map(|a| a.replace(FILE_PLACEHOLDER, &path))
        .collect();
    if !args.iter().any(|a| a.contains(FILE_PLACEHOLDER)) {
        substituted.push(path);
    }
    let mut cmd = std::process::Command::new(&substituted[0]);
    cmd.args(&substituted[1..]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(cmd)
}

fn truncate_detail(detail: &str) -> String {
    let mut s: String = detail.chars().take(MAX_SCAN_DETAIL_CHARS).collect();
    if detail.chars().count() > MAX_SCAN_DETAIL_CHARS {
        s.push('…');
    }
    s.trim().to_string()
}

/// The first non-empty line(s) of scanner output, for the threat
/// detail. Output is capped before reading so a chatty scanner
/// cannot fill memory.
fn output_detail(output: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !stderr.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    let first: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(3)
        .collect();
    if first.is_empty() {
        String::new()
    } else {
        first.join(" ")
    }
}

/// Does scanner output name a threat even though the scanner
/// exited 0? ClamAV-style `… FOUND`, or the words `infected`,
/// `threat found`, `virus found` — matched case-insensitively.
/// A plain "clean" / "OK" / empty output is not a threat.
pub fn output_names_threat(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    if lower.contains("infected") || lower.contains("threat found") || lower.contains("virus found")
    {
        return true;
    }
    // ClamAV-style `path: Name FOUND` — line-based, so a plain
    // "file not found" error from a scanner is not a threat.
    lower.lines().any(|line| {
        line.contains("found") && !line.contains("not found") && !line.contains("no threat")
    })
}

/// Run the configured scanner command against `file_path` and
/// report the result. Never returns an error for a scan outcome —
/// a threat, a failure and a timeout are all results; only a
/// caller error (bad template) is an `Err`. The file must exist:
/// scanning a missing file is a failure, not a clean pass.
///
/// The wait runs on a blocking thread: the scanner's pipes are
/// drained by two threads (so a chatty scanner cannot deadlock
/// on a full pipe) while the main thread polls `try_wait` — on
/// timeout the scanner is killed and waited for, so no orphan
/// scanner keeps running and the app never hangs on it.
pub async fn run_scan(command_template: &str, file_path: &Path, timeout_secs: i64) -> ScanResult {
    let template = command_template.trim();
    if template.is_empty() {
        return ScanResult::not_configured("No antivirus scanner is configured in Settings.");
    }
    if !file_path.is_file() {
        return ScanResult::failed(format!(
            "The file to scan does not exist: {}",
            file_path.display()
        ));
    }
    let mut cmd = match build_command(template, file_path) {
        Ok(c) => c,
        Err(e) => return ScanResult::failed(e.to_string()),
    };
    let timeout = Duration::from_secs(timeout_secs.max(1) as u64);
    match task::spawn_blocking(move || run_scan_blocking(&mut cmd, timeout)).await {
        Ok(result) => result,
        Err(e) => ScanResult::failed(format!("The antivirus scan could not run: {e}")),
    }
}

fn run_scan_blocking(cmd: &mut std::process::Command, timeout: Duration) -> ScanResult {
    use std::io::Read;
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ScanResult::failed(format!(
                "Could not start the antivirus scanner: {e}. Check the command in Settings."
            ))
        }
    };
    // Drain both pipes on their own threads while the wait polls,
    // so output can never fill a pipe and stall the scanner.
    let out_handle = std::thread::spawn({
        let mut pipe = child.stdout.take();
        move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe.take() {
                let _ = p.read_to_end(&mut buf);
            }
            buf.truncate(MAX_SCAN_OUTPUT_BYTES);
            buf
        }
    });
    let err_handle = std::thread::spawn({
        let mut pipe = child.stderr.take();
        move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe.take() {
                let _ = p.read_to_end(&mut buf);
            }
            buf.truncate(MAX_SCAN_OUTPUT_BYTES);
            buf
        }
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = out_handle.join().unwrap_or_default();
                let stderr = err_handle.join().unwrap_or_default();
                let output = std::process::Output {
                    status,
                    stdout,
                    stderr,
                };
                return classify_output(&output);
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Do NOT join the drain threads here: the
                    // scanner's children (e.g. a `sleep` the
                    // scanner spawned) may still hold the pipes
                    // open, so a join would wait for them too —
                    // exactly the hang the timeout exists to
                    // avoid. The threads finish on their own when
                    // those children exit; they hold no resources
                    // the caller needs.
                    return ScanResult::timed_out(timeout.as_secs() as i64);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                return ScanResult::failed(format!("Could not read the scanner's result: {e}"));
            }
        }
    }
}

/// Classify a finished scanner run: exit 0 with no threat in the
/// output is clean; a non-zero exit, or an exit-0 run whose output
/// names a threat, is infected — the detail is the scanner's own
/// first line(s), never an invented threat name.
fn classify_output(output: &std::process::Output) -> ScanResult {
    let detail = output_detail(output);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        if output_names_threat(&combined) {
            ScanResult::infected(if detail.is_empty() {
                "The scanner reported a threat but gave no detail.".to_string()
            } else {
                detail
            })
        } else {
            ScanResult::clean()
        }
    } else {
        ScanResult::infected(if detail.is_empty() {
            format!(
                "The scanner exited with status {} — it reported the file as a threat.",
                output.status
            )
        } else {
            detail
        })
    }
}

/// The Settings > Downloads "Test" button: run the (possibly
/// unsaved) scanner command against a small throwaway file and
/// report the result. The test file is deleted afterwards whether
/// the scan passes or fails, and it contains only a harmless
/// marker line — never a real file, never a malware sample.
pub async fn test_scanner(command_template: &str, timeout_secs: i64) -> Result<ScanResult> {
    validate_scan_timeout(timeout_secs)?;
    if command_template.trim().is_empty() {
        return Err(EngineError::InvalidState(
            "Enter a scanner command first — there is nothing to test".to_string(),
        ));
    }
    let dir = std::env::temp_dir().join(format!("fetchline-av-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let test_file = dir.join("fetchline-scanner-test.txt");
    std::fs::write(
        &test_file,
        "Fetchline antivirus test file — a harmless marker, not a real download.\n",
    )?;
    let result = run_scan(command_template, &test_file, timeout_secs).await;
    let _ = std::fs::remove_file(&test_file);
    let _ = std::fs::remove_dir(&dir);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_command_handles_quotes_and_spaces() {
        assert_eq!(
            split_command(r#""C:\Program Files\Scanner\scan.exe" -Scan "{file}""#).unwrap(),
            vec![
                r#"C:\Program Files\Scanner\scan.exe"#.to_string(),
                "-Scan".to_string(),
                "{file}".to_string(),
            ]
        );
        assert_eq!(
            split_command("scan.exe --file '{file}'").unwrap(),
            vec!["scan.exe", "--file", "{file}"]
        );
        assert_eq!(
            split_command("  scan.exe   -q  ").unwrap(),
            vec!["scan.exe", "-q"]
        );
        // BUG-034: an unquoted Windows path keeps its backslashes
        // (the old code stripped every one, so the program could
        // never be found and every scan failed at spawn).
        assert_eq!(
            split_command(r#"C:\Scanner\scan.exe {file}"#).unwrap(),
            vec![r#"C:\Scanner\scan.exe"#.to_string(), "{file}".to_string()]
        );
        assert!(split_command("").is_err());
        assert!(split_command("\"unterminated").is_err());
    }

    #[test]
    fn scan_timeout_bounds_are_rejected_not_clamped() {
        assert!(validate_scan_timeout(5).is_ok());
        assert!(validate_scan_timeout(60).is_ok());
        assert!(validate_scan_timeout(600).is_ok());
        assert!(validate_scan_timeout(4).is_err());
        assert!(validate_scan_timeout(601).is_err());
        assert!(validate_scan_timeout(0).is_err());
    }

    #[test]
    fn output_threat_markers() {
        assert!(output_names_threat("file.txt: FakeThreat FOUND"));
        assert!(output_names_threat("1 file infected"));
        assert!(output_names_threat("Threat found in file"));
        assert!(!output_names_threat("clean"));
        assert!(!output_names_threat(""));
        assert!(!output_names_threat("OK - no threats"));
        assert!(!output_names_threat("file not found"));
    }
}
