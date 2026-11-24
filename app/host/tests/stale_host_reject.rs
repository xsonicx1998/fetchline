// Round 88.2 — process-level: the stale-host mechanism.
//
// The relay broke live in Rounds 88/88.1 with ZERO "relay"
// lines in host.log, while the watcher kept working. The
// mechanism: the VPS builds rebuilt the host crate but never
// copied it to the sidecar slot, so the installer bundled a
// pre-88 host with no `RelayCapture` variant. A host that does
// not know a message type fails `serde_json::from_value` and
// answers with a framed error — and writes NOTHING to host.log
// on that path (see main.rs: the `Err(e)` arm replies and
// continues, with no `log_line` call). The extension then
// throws inside `handToFetchline` and shows the error only in
// the re-opened dropdown on the VIDEO page, which a retester
// watching the desktop never sees — hence no dialog, no row,
// and no trace anywhere.
//
// These tests pin that mechanism with a variant no host knows
// (`relay_capture_v2`), and pin the new startup identity line
// that makes a stale host detectable from host.log.
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn frame(value: &serde_json::Value) -> Vec<u8> {
    let data = value.to_string().into_bytes();
    let mut f = (data.len() as u32).to_le_bytes().to_vec();
    f.extend_from_slice(&data);
    f
}

fn tmpdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fetchline-host-stale-test-{}-{}",
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

#[test]
fn unknown_variant_gets_a_framed_error_and_no_host_log_line() {
    let dir = tmpdir();
    let db = dir.join("app.db");
    let log_path = dir.join("host.log");
    let mut host = HostChild::spawn(&db);
    // A message type this host does not know — exactly what a
    // pre-88 host sees when it receives `relay_capture`.
    let reply = host.send(&serde_json::json!({
        "type": "relay_capture_v2",
        "url": "https://cdn.example.com/v/movie.mp4",
        "finalUrl": "https://cdn.example.com/v/movie.mp4?sig=abc"
    }));
    assert_eq!(
        reply.get("ok"),
        Some(&serde_json::Value::Bool(false)),
        "an unknown variant must be refused: {reply}"
    );
    let error = reply
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        error.contains("Could not understand"),
        "the refusal must carry the framed error: {reply}"
    );
    assert!(
        error.contains("relay_capture_v2"),
        "the error must name the unknown variant: {reply}"
    );
    // The host stays alive and answers a ping afterwards.
    let pong = host.send(&serde_json::json!({ "type": "ping" }));
    assert_eq!(pong.get("ok"), Some(&serde_json::Value::Bool(true)));
    host.finish();
    // The silent half of the mechanism: host.log has the
    // startup line and NOTHING about the refused variant —
    // this is why the live relay failure left zero "relay"
    // lines in either log.
    let log = std::fs::read_to_string(&log_path).expect("host.log must exist");
    assert!(
        log.contains("fetchline-host: started"),
        "host.log must have the startup line: {log}"
    );
    assert!(
        !log.contains("relay_capture_v2"),
        "the refused variant must leave no trace in host.log: {log}"
    );
}

#[test]
fn startup_line_carries_the_build_identity() {
    let dir = tmpdir();
    let db = dir.join("app.db");
    let log_path = dir.join("host.log");
    let mut host = HostChild::spawn(&db);
    let pong = host.send(&serde_json::json!({ "type": "ping" }));
    assert_eq!(pong.get("ok"), Some(&serde_json::Value::Bool(true)));
    host.finish();
    let log = std::fs::read_to_string(&log_path).expect("host.log must exist");
    let first = log.lines().next().unwrap_or_default();
    assert!(
        first.contains("head=") && first.contains("built="),
        "the first host.log line must carry the build identity (head=… built=…): {first}"
    );
}

#[test]
fn get_settings_reply_carries_relay_enabled() {
    // Round 88.3 — the host never sent `relay_enabled` in its
    // get_settings reply, so the extension's capture gate read
    // `undefined` and was stuck ON: the app's relay checkbox
    // could never gate capture. A fresh database defaults the
    // relay ON, and the reply must carry that.
    let dir = tmpdir();
    let db = dir.join("app.db");
    let mut host = HostChild::spawn(&db);
    let reply = host.send(&serde_json::json!({ "type": "get_settings" }));
    assert_eq!(
        reply.get("ok"),
        Some(&serde_json::Value::Bool(true)),
        "get_settings must answer ok: {reply}"
    );
    assert_eq!(
        reply.get("relay_enabled"),
        Some(&serde_json::Value::Bool(true)),
        "get_settings must carry the relay switch (fresh DB defaults ON): {reply}"
    );
    host.finish();
}
