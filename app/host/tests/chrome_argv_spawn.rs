// BUG-043 regression, process-level: the host used to exit(2) on
// Chrome's native-messaging launch arguments (the extension origin
// + --parent-window), so a real browser spawn never got a reply.
// This spawns the built host exactly the way Chrome does and
// requires a framed pong — then EOF, and the host must exit
// cleanly. Integration test (tests/): CARGO_BIN_EXE_* is only set
// for integration tests, not for unit tests inside main.rs.
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn frame_ping() -> Vec<u8> {
    let data = serde_json::json!({"type": "ping"}).to_string().into_bytes();
    let mut f = (data.len() as u32).to_le_bytes().to_vec();
    f.extend_from_slice(&data);
    f
}

#[test]
fn host_answers_ping_when_spawned_with_chrome_argv() {
    let dir = std::env::temp_dir().join(format!(
        "fetchline-host-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("app.db");
    let mut child = Command::new(env!("CARGO_BIN_EXE_fetchline-host"))
        .arg("chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/")
        .arg("--parent-window=0")
        .arg("--db-path")
        .arg(&db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn host");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    // Bounded read on a thread: the reply (or the child's death)
    // must arrive within 15 s, or the test fails instead of hanging.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let res = (|| -> std::io::Result<Vec<u8>> {
            let mut hdr = [0u8; 4];
            stdout.read_exact(&mut hdr)?;
            let n = u32::from_le_bytes(hdr) as usize;
            let mut buf = vec![0u8; n];
            stdout.read_exact(&mut buf)?;
            Ok(buf)
        })();
        let _ = tx.send(res);
    });
    stdin.write_all(&frame_ping()).unwrap();
    stdin.flush().unwrap();
    let reply = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("host did not reply in 15 s — it probably exited on Chrome's argv (BUG-043)")
        .expect("read reply frame");
    let value: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(
        value.get("ok"),
        Some(&serde_json::Value::Bool(true)),
        "ping reply not ok: {value}"
    );
    assert_eq!(
        value.get("type"),
        Some(&serde_json::Value::String("pong".to_string())),
        "ping reply not pong: {value}"
    );
    drop(stdin); // EOF: the host must exit cleanly, code 0.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().unwrap() {
            Some(status) => {
                assert!(status.success(), "host exit code: {status}");
                break;
            }
            None => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    panic!("host did not exit after stdin EOF");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
