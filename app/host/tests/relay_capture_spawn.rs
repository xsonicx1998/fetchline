// Round 88 (relay) — process-level: the host binary the
// browser actually spawns must accept a `relay_capture` frame
// and queue it, and refuse one with a bad cookie, exactly like
// the in-process `handle_host_message` tests prove. Modelled on
// `chrome_argv_spawn.rs` (same Chrome-style argv, same framed
// reply read with a timeout, clean EOF exit).
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use fetchline_engine::Store;

fn frame(value: &serde_json::Value) -> Vec<u8> {
    let data = value.to_string().into_bytes();
    let mut f = (data.len() as u32).to_le_bytes().to_vec();
    f.extend_from_slice(&data);
    f
}

fn tmpdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fetchline-host-relay-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct HostChild {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<std::io::Result<Vec<u8>>>,
}

impl HostChild {
    fn spawn(db: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_fetchline-host"))
            .arg("chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/")
            .arg("--parent-window=0")
            .arg("--db-path")
            .arg(db)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn host");
        let stdin = child.stdin.take().unwrap();
        // stdout is owned by the reader thread for the whole
        // run — the struct never holds it.
        let mut stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || loop {
            let res = (|| -> std::io::Result<Vec<u8>> {
                let mut hdr = [0u8; 4];
                stdout.read_exact(&mut hdr)?;
                let n = u32::from_le_bytes(hdr) as usize;
                let mut buf = vec![0u8; n];
                stdout.read_exact(&mut buf)?;
                Ok(buf)
            })();
            if tx.send(res).is_err() {
                break;
            }
        });
        Self { child, stdin, rx }
    }

    fn send(&mut self, value: &serde_json::Value) -> serde_json::Value {
        self.stdin.write_all(&frame(value)).unwrap();
        self.stdin.flush().unwrap();
        let reply = self
            .rx
            .recv_timeout(Duration::from_secs(15))
            .expect("host did not reply in 15 s")
            .expect("read reply frame");
        serde_json::from_slice(&reply).unwrap()
    }

    fn finish(mut self) {
        drop(self.stdin);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait().unwrap() {
                Some(status) => {
                    assert!(status.success(), "host exit code: {status}");
                    return;
                }
                None => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "host did not exit on EOF"
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
}

fn relay_capture(cookies: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "relay_capture",
        "url": "https://cdn.example.com/v/movie.mp4",
        "finalUrl": "https://cdn.example.com/v/movie.mp4?sig=abc",
        "page_url": "https://example.com/watch/123",
        "referrer": "https://example.com/watch/123",
        "content_type": "video/mp4",
        "content_length": 238000000,
        "filename": "movie.mp4",
        "kind": "media",
        "headers": {
            "User-Agent": "Mozilla/5.0",
            "Referer": "https://example.com/watch/123",
            "Cookie": "SID=smuggled"
        },
        "cookies": cookies
    })
}

#[test]
fn host_spawns_and_queues_a_relay_capture() {
    let dir = tmpdir();
    let db = dir.join("app.db");
    let mut host = HostChild::spawn(&db);
    // The relay master switch defaults ON on a fresh database.
    let reply = host.send(&relay_capture(serde_json::json!([
        { "name": "session", "value": "test-dummy-value", "domain": "example.com", "path": "/" }
    ])));
    assert_eq!(
        reply.get("ok"),
        Some(&serde_json::Value::Bool(true)),
        "relay_capture reply not ok: {reply}"
    );
    assert_eq!(reply.get("type").unwrap(), "queued");
    let pending_link_id = reply
        .get("pending_link_id")
        .and_then(|v| v.as_str())
        .expect("the reply must carry the pending row id")
        .to_string();
    // A relay_capture with a bad cookie is refused, not queued.
    let bad = host.send(&relay_capture(serde_json::json!([
        { "name": "session", "value": "abc; admin=1" }
    ])));
    assert_eq!(
        bad.get("ok"),
        Some(&serde_json::Value::Bool(false)),
        "a bad cookie must be refused by the spawned host: {bad}"
    );
    host.finish();
    // The row is really in the database the host wrote.
    let store = Store::open(&db).unwrap();
    let row = store
        .get_pending_link(&pending_link_id)
        .unwrap()
        .expect("the relay row must exist");
    assert_eq!(row.source, "relay");
    assert_eq!(row.url, "https://cdn.example.com/v/movie.mp4?sig=abc");
    assert_eq!(
        row.source_url.as_deref(),
        Some("https://cdn.example.com/v/movie.mp4")
    );
    let headers: serde_json::Value = serde_json::from_str(row.headers.as_deref().unwrap()).unwrap();
    assert!(
        headers.get("Cookie").is_none(),
        "the smuggled Cookie key must be stripped: {headers}"
    );
    assert_eq!(store.list_pending_links(Some("pending")).unwrap().len(), 1);
}
