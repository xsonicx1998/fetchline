fn main() {
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
    if std::path::Path::new("../../.git/HEAD").exists() {
        println!("cargo:rerun-if-changed=../../.git/HEAD");
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
