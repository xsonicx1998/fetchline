//! Round 71 (B5) — a server that refuses the 4th and later
//! concurrent connections (OVH rate-limits parallel range
//! requests from one IP: 5/8 ok, 3x 429 in the live probe) must
//! not fail the download: the refused segments defer and retry
//! under the ratcheted concurrency instead of failing the whole
//! run (the old `abort_all` on one segment failure).

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;
use std::time::Duration;

use mock::*;

use fetchline_engine::{DownloadManager, Store};

#[tokio::test]
async fn download_completes_when_server_limits_concurrency_to_three() {
    // 2 MiB file, 8 segments of 256 KiB (4 chunks each). Each
    // chunk costs 2 s, so a segment takes ~6 s — longer than the
    // deferral retry schedule (0.5 + 1.5 + 3.0 = 5 s), so a
    // refused segment exhausts all 4 attempts while the first 3
    // are still transferring, and MUST defer: without the
    // deferral the 4th failed attempt would fail the download
    // (the old code failed the whole run as soon as one segment
    // exhausted its retries).
    let server = MockServer::start(MockConfig {
        chunk: 65536,
        delay: Duration::from_millis(2000),
        size_bytes: Some(2 * 1024 * 1024),
        max_concurrent_gets: Some(3),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("engine.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 8);
    let (id, _) = mgr
        .add_download(
            &server.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 60).await,
        "download did not complete under a 3-connection limit"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        std::fs::read(&row.save_path).unwrap(),
        payload(2 * 1024 * 1024)
    );
    // The limit actually fired — not a vacuous pass.
    assert!(
        server.rate_limited() > 0,
        "the mock never refused a request"
    );
    // ...and the refused segments went through the deferral
    // path (not just the retry loop): the engine log must carry
    // a deferral line.
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.contains("deferred"),
        "no segment deferral in the engine log:\n{log}"
    );
}
