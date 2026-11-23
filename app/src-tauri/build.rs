#[path = "host_sidecar.rs"]
mod host_sidecar;

fn main() {
    // Round 88.2 — sync the host sidecar BEFORE the bundle
    // step: `tauri build` bundles `binaries/fetchline-host` but
    // never rebuilds the host crate, so a `cargo build
    // --release -p fetchline-host` that is never copied here
    // silently ships a STALE host. That is what broke the relay
    // live in Rounds 88/88.1 (both VPS builds rebuilt the host
    // but never copied it, so the installer bundled a pre-88
    // host with no `RelayCapture` — every relay_capture failed
    // its serde decode with no trace in either log). The copy
    // is mtime-guarded, warns loudly when the host was never
    // built, and never fails the build on its own.
    if let Ok(target) = std::env::var("TARGET") {
        let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let outcome = host_sidecar::sync_host_sidecar(&manifest_dir, &target);
        println!("cargo:warning={}", host_sidecar::outcome_line(&outcome));
    }
    tauri_build::build();
    // Round 87.5 (item 11) — build identity, baked at build
    // time so startup.log can name the exact binary under test
    // (`build: head=<sha> built=<UTC> debug-hooks=<on|off>
    // version=<x>`, logged in `build_manager`). The head comes
    // from git when this is a checkout (the VPS and CI both
    // build from one), else the FETCHLINE_BUILD_HEAD env var,
    // else "unknown" — never a build failure. The UTC time is
    // formatted here with a std-only civil-from-days conversion
    // (Howard Hinnant's algorithm), so no new dependency.
    println!("cargo:rustc-env=FETCHLINE_BUILD_HEAD={}", build_head());
    println!(
        "cargo:rustc-env=FETCHLINE_BUILD_TIME_UTC={}",
        build_time_utc()
    );
    println!("cargo:rerun-if-env-changed=FETCHLINE_BUILD_HEAD");
    // Re-bake the head when the checkout moves (only when this
    // IS a checkout — the Linux dev copy is not a git repo).
    // Round 88.1 — tracking `.git/HEAD` alone was the stale
    // build line: HEAD only names the branch (`ref:
    // refs/heads/main`) and does NOT change on `git pull`, so
    // cargo reused the cached build-script output and the Round
    // 88 binary reported the 87.8 head (352aba6) and time.
    // Track the ref HEAD points at (and packed-refs, in case
    // the ref is packed) so a pull invalidates the cache.
    track_git_head();
}

/// Emit `cargo:rerun-if-changed` for the files that actually
/// move when the checkout moves: HEAD itself (covers a
/// detached HEAD, where it holds the SHA), plus the branch ref
/// it points at and `packed-refs` (covers a packed ref).
/// Handles `.git` being a directory (normal checkout) or a
/// file (`gitdir: <path>`, worktree/submodule). Any failure is
/// silent — the build identity falls back to "unknown", never
/// a build failure.
fn track_git_head() {
    let dotgit = std::path::Path::new("../../.git");
    let git_dir = if dotgit.is_dir() {
        dotgit.to_path_buf()
    } else if let Ok(text) = std::fs::read_to_string(dotgit) {
        match text.trim().strip_prefix("gitdir:") {
            Some(path) => std::path::PathBuf::from(path.trim()),
            None => return,
        }
    } else {
        return;
    };
    let head_path = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head_path.display());
    let Ok(head) = std::fs::read_to_string(&head_path) else {
        return;
    };
    if let Some(refname) = head.trim().strip_prefix("ref:") {
        let refname = refname.trim();
        let ref_path = git_dir.join(refname);
        println!("cargo:rerun-if-changed={}", ref_path.display());
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join("packed-refs").display()
        );
    }
}

fn build_head() -> String {
    if let Ok(head) = std::env::var("FETCHLINE_BUILD_HEAD") {
        let head = head.trim().to_string();
        if !head.is_empty() {
            return head;
        }
    }
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    {
        if output.status.success() {
            let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !head.is_empty() {
                return head;
            }
        }
    }
    "unknown".to_string()
}

fn build_time_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// Unix seconds -> "YYYY-MM-DDTHH:MM:SSZ" (Howard Hinnant's
/// civil-from-days; std-only).
fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let (h, m, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let mo = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    if mo <= 2 {
        y += 1;
    }
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}
