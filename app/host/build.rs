// Round 88.2 — host build identity, baked at build time so
// host.log can name the exact host binary the browser spawned
// (`fetchline-host: started (head=<sha> built=<UTC>)`).
//
// Why: the relay broke live in Rounds 88/88.1 with ZERO relay
// lines in host.log, and there was NO way to tell from the VPS
// which host was actually installed — the installed host had
// no identity at all, and the VPS builds had rebuilt the host
// crate without copying it to the sidecar slot, so the
// installer bundled a stale pre-88 host. With this line, the
// first line of host.log proves the installed host's head; if
// it is old or missing, the host is stale and must be rebuilt
// + recopied + reinstalled before any retest means anything.
//
// Mirrors the app's build.rs (Round 87.5/88.1): the head comes
// from git when this is a checkout, else FETCHLINE_HOST_BUILD_HEAD,
// else "unknown"; the UTC time uses the same std-only
// civil-from-days conversion (Howard Hinnant's algorithm).
// Never a build failure. The git-ref tracking is the 88.1
// lesson — `.git/HEAD` alone never changes on `git pull`, so
// the branch ref and packed-refs are tracked too.
fn main() {
    println!("cargo:rustc-env=FETCHLINE_HOST_BUILD_HEAD={}", build_head());
    println!(
        "cargo:rustc-env=FETCHLINE_HOST_BUILD_TIME_UTC={}",
        build_time_utc()
    );
    println!("cargo:rerun-if-env-changed=FETCHLINE_HOST_BUILD_HEAD");
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
    if let Ok(head) = std::env::var("FETCHLINE_HOST_BUILD_HEAD") {
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
