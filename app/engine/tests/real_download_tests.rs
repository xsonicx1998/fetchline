//! Real-internet download tests (CI: Windows runner, dedicated step).
//!
//! These are the ONLY engine tests that touch the public internet.
//! They download Fetchline's own LICENSE from a PINNED commit via
//! raw.githubusercontent.com through the REAL DownloadManager
//! (probe + add_download + start + segments — no reimplementation)
//! and verify the bytes byte-for-byte.
//!
//! Pinned file: LICENSE at commit 7255dff9 (the BUG-029 fix commit),
//! 1,079 bytes, SHA-256
//! c2ef3696eecd8d51c0e615afe9437e5d620abcd9272b9d9f4bcbbd1e4a95f793
//! (verified against the repo copy 2026-11-15: the pinned URL serves
//! byte-identical content).
//!
//! If the repo is ever renamed, only the URL changes — the LICENSE
//! text (and therefore the hash) stays the same. Update PINNED_URL
//! and keep EXPECTED_SHA256. A 404 or a hash mismatch fails loudly:
//! either the pin moved (fix the URL) or the download corrupted
//! bytes (a real bug — never weaken the assertion to fit).
//!
//! The tests need internet and take seconds (1 KB file). They run
//! in the full suite too, and in CI they also run alone in a
//! dedicated step that FAILS the run on failure (the step-8
//! `cargo test` runs with continue-on-error and must not hide
//! this one).
//!
//! Env gate: the tests only run when `FETCHLINE_REAL_DOWNLOAD_TEST`
//! is set to `1` (the CI step sets it). Without it they print
//! SKIPPED and pass. The gate exists because this Linux dev VM sits
//! behind a TLS-intercepting egress proxy (its `Hatch Sandbox
//! Egress CA` is in the system store, so curl trusts it, but the
//! engine's rustls client uses compiled-in webpki roots and
//! rejects it with `InvalidCertificate(UnknownIssuer)`). That is a
//! property of this VM, not of the download code — the Windows
//! runner has direct internet and no intercepting proxy, so the
//! tests run for real there. Never "fix" a local skip by weakening
//! TLS verification in the test: the engine must keep verifying
//! real certificates.

#![allow(dead_code)] // the shared mock also serves the other test files; this file only uses tmpdir/wait_for_status

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;

use mock::*;
use sha2::{Digest, Sha256};

use fetchline_engine::{probe, DownloadManager, Store};

/// Pinned LICENSE URL (commit 7255dff9, on main).
const PINNED_URL: &str = "https://raw.githubusercontent.com/mohandksonic-ui/fetchline/7255dff9ec2863b16bda2772a839654b51274c32/LICENSE";
/// SHA-256 of the repo's LICENSE (1,079 bytes, MIT text).
const EXPECTED_SHA256: &str = "c2ef3696eecd8d51c0e615afe9437e5d620abcd9272b9d9f4bcbbd1e4a95f793";
/// The file is tiny; 90 s is generous even on a cold CI runner.
const WAIT_SECS: u64 = 90;

/// True only when the real-internet tests are enabled
/// (`FETCHLINE_REAL_DOWNLOAD_TEST=1`, set by the CI step). Without
/// it the tests print SKIPPED and pass — see the module docs.
fn real_download_enabled() -> bool {
    std::env::var("FETCHLINE_REAL_DOWNLOAD_TEST").as_deref() == Ok("1")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let mut out = String::with_capacity(64);
    for b in hasher.finalize() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[tokio::test]
async fn real_download_of_pinned_license_matches_sha256() {
    if !real_download_enabled() {
        println!("SKIPPED: set FETCHLINE_REAL_DOWNLOAD_TEST=1 to run the real-internet download (see module docs)");
        return;
    }
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(
            PINNED_URL,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", WAIT_SECS).await,
        "real download of {PINNED_URL} did not complete within {WAIT_SECS}s — check the download row for its status/error"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        row.save_path.ends_with("LICENSE"),
        "save path should end in LICENSE, got {}",
        row.save_path
    );
    let bytes = std::fs::read(&row.save_path).unwrap();
    assert_eq!(
        bytes.len(),
        1079,
        "downloaded LICENSE must be 1,079 bytes, got {}",
        bytes.len()
    );
    assert_eq!(
        sha256_hex(&bytes),
        EXPECTED_SHA256,
        "downloaded LICENSE bytes do not match the pinned SHA-256 — the download corrupted bytes or the pin moved"
    );
}

#[tokio::test]
async fn real_probe_of_pinned_license_reports_size() {
    if !real_download_enabled() {
        println!("SKIPPED: set FETCHLINE_REAL_DOWNLOAD_TEST=1 to run the real-internet probe (see module docs)");
        return;
    }
    let client = reqwest::Client::new();
    let info = probe(&client, PINNED_URL).await.unwrap();
    assert_eq!(
        info.size,
        Some(1079),
        "probe of the pinned LICENSE must report 1,079 bytes, got {:?}",
        info.size
    );
}
