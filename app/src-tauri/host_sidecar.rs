// Round 88.2 — the host sidecar sync.
//
// The desktop app ships the native-messaging host as a Tauri
// `externalBin` (`binaries/fetchline-host` in tauri.conf.json):
// `tauri build` bundles `binaries/fetchline-host-<triple>.exe`
// from this directory, and the installer drops it next to the
// app exe as `fetchline-host.exe`. The host is a SEPARATE Cargo
// package (`app/host`), so `tauri build` does NOT rebuild it —
// a `cargo build --release -p fetchline-host` that is never
// copied here silently ships a STALE host.
//
// That is exactly what broke the relay live in Rounds 88 and
// 88.1: both VPS builds rebuilt the host crate but never copied
// it, so the installer bundled a pre-Round-88 host with no
// `RelayCapture` variant. A `relay_capture` then fails the
// host's serde decode ("unknown variant") and is answered with
// a framed error that is NOT written to host.log — so the live
// failure left zero "relay" lines in either log, while the
// watcher (`download_link`, an ancient message type) kept
// working and the popup still showed "Connected" (ping is
// ancient too). The Round 71 A3 safeguard (copy + print
// LastWriteTime at every build) was procedural and was
// forgotten twice; this module makes the copy automatic.
//
// Called from build.rs (which runs before the bundle step, so
// the fresh copy is in place when Tauri resolves externalBin).
// Copy-if-newer only: a build that did not touch the host
// costs one stat call. When the host exe was never built, this
// warns loudly but does NOT fail the build — the bundle step
// still fails on its own if the sidecar is missing entirely,
// and a `cargo check` flow that never bundles must not break.
// Windows only: the sidecar is a Windows artifact.
use std::path::{Path, PathBuf};

/// The sidecar name Tauri resolves for `binaries/fetchline-host`
/// on Windows x86_64.
pub const SIDECAR_NAME: &str = "fetchline-host-x86_64-pc-windows-msvc.exe";

/// Outcome of one sync, for the build log.
#[derive(Debug, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Fresh host copied over the sidecar.
    Copied,
    /// Sidecar already as new as the host build — nothing to do.
    Current,
    /// No host exe was built yet (or the copy failed): the build
    /// log carries a warning; the bundle step still fails on its
    /// own if the sidecar is missing entirely.
    Warning(String),
}

/// Copy the freshly built host exe into this crate's `binaries/`
/// sidecar slot when it is newer than what is there.
/// `manifest_dir` is this crate's directory (CARGO_MANIFEST_DIR);
/// `target_triple` is the build target (TARGET env var).
pub fn sync_host_sidecar(manifest_dir: &Path, target_triple: &str) -> SyncOutcome {
    if !target_triple.contains("windows") {
        return SyncOutcome::Current;
    }
    let host_exe = manifest_dir
        .join("..")
        .join("host")
        .join("target")
        .join("release")
        .join("fetchline-host.exe");
    let sidecar = manifest_dir.join("binaries").join(SIDECAR_NAME);
    sync_host_sidecar_paths(&host_exe, &sidecar)
}

fn sync_host_sidecar_paths(host_exe: &Path, sidecar: &Path) -> SyncOutcome {
    let Ok(host_meta) = std::fs::metadata(host_exe) else {
        return SyncOutcome::Warning(format!(
            "fetchline-host.exe was not built yet ({} missing) — the installer will bundle whatever sidecar is already in binaries/, which may be stale; build the host first: cargo build --release -p fetchline-host",
            host_exe.display()
        ));
    };
    let Ok(host_mtime) = host_meta.modified() else {
        return SyncOutcome::Warning(format!(
            "could not read the mtime of {} — sidecar left as-is",
            host_exe.display()
        ));
    };
    if let Ok(side_meta) = std::fs::metadata(sidecar) {
        if let Ok(side_mtime) = side_meta.modified() {
            if side_mtime >= host_mtime {
                return SyncOutcome::Current;
            }
        }
    }
    if let Some(parent) = sidecar.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return SyncOutcome::Warning(format!(
                "could not create {}: {e} — sidecar left as-is",
                parent.display()
            ));
        }
    }
    match std::fs::copy(host_exe, sidecar) {
        Ok(_) => SyncOutcome::Copied,
        Err(e) => SyncOutcome::Warning(format!(
            "could not copy {} to {}: {e} — sidecar left as-is (the installer may bundle a stale host)",
            host_exe.display(),
            sidecar.display()
        )),
    }
}

/// Format the outcome for the build log (one line).
pub fn outcome_line(outcome: &SyncOutcome) -> String {
    match outcome {
        SyncOutcome::Copied => {
            "host sidecar: copied the freshly built fetchline-host.exe into binaries/".to_string()
        }
        SyncOutcome::Current => {
            "host sidecar: binaries/ already holds the current host build".to_string()
        }
        SyncOutcome::Warning(msg) => format!("host sidecar WARNING: {msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fetchline-host-sidecar-test-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn touch_newer(path: &Path) {
        // Best-effort mtime bump: rewrite the file after a short
        // sleep so its mtime is strictly newer (filesystems with
        // coarse timestamps need the delay).
        std::thread::sleep(Duration::from_millis(1100));
        let bytes = std::fs::read(path).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn copies_a_fresh_host_over_a_missing_sidecar() {
        let dir = tmpdir("missing");
        let host = dir.join("host.exe");
        let sidecar = dir.join("binaries").join(SIDECAR_NAME);
        write(&host, b"fresh-host");
        assert_eq!(
            sync_host_sidecar_paths(&host, &sidecar),
            SyncOutcome::Copied
        );
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"fresh-host");
    }

    #[test]
    fn skips_the_copy_when_the_sidecar_is_current() {
        let dir = tmpdir("current");
        let host = dir.join("host.exe");
        let sidecar = dir.join("binaries").join(SIDECAR_NAME);
        write(&host, b"same-host");
        write(&sidecar, b"same-host");
        touch_newer(&sidecar);
        assert_eq!(
            sync_host_sidecar_paths(&host, &sidecar),
            SyncOutcome::Current
        );
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"same-host");
    }

    #[test]
    fn recopies_when_the_host_is_newer_than_the_sidecar() {
        let dir = tmpdir("stale");
        let host = dir.join("host.exe");
        let sidecar = dir.join("binaries").join(SIDECAR_NAME);
        write(&host, b"old-host");
        write(&sidecar, b"old-host");
        // Sleep past the filesystem timestamp granularity, then
        // write the new host bytes: its mtime is now newer.
        std::thread::sleep(Duration::from_millis(1100));
        write(&host, b"new-host");
        assert_eq!(
            sync_host_sidecar_paths(&host, &sidecar),
            SyncOutcome::Copied
        );
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"new-host");
    }

    #[test]
    fn warns_but_does_not_fail_when_the_host_was_never_built() {
        let dir = tmpdir("nobuild");
        let host = dir.join("host.exe");
        let sidecar = dir.join("binaries").join(SIDECAR_NAME);
        match sync_host_sidecar_paths(&host, &sidecar) {
            SyncOutcome::Warning(msg) => assert!(msg.contains("was not built yet"), "{msg}"),
            other => panic!("expected a warning, got {other:?}"),
        }
        assert!(!sidecar.exists(), "no sidecar must appear from nothing");
    }

    #[test]
    fn non_windows_targets_are_a_no_op() {
        let dir = tmpdir("linux");
        assert_eq!(
            sync_host_sidecar(&dir, "x86_64-unknown-linux-gnu"),
            SyncOutcome::Current
        );
    }
}
