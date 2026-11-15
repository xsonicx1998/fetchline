//! Proxy finish (S09 row 18): FTP transport + NTLM proxy auth.
//!
//! Every test serves its own fixture on 127.0.0.1 — a local FTP
//! server (`FtpServer` below) and a local stub HTTP proxy that
//! demands NTLM (`NtlmProxyServer` below). No public server is ever
//! used, and no password here is real: all credentials are
//! test-only dummies (`alice` / `hunter2`, `proxyuser` /
//! `proxypass`), never logged and never stored outside the test.
//!
//! The NTLM stub verifies a Type 3 by recomputing its proof with
//! the engine's own NTLM helpers — that pins the handshake *flow*
//! (Type 1 -> Type 2 -> Type 3, then the request is forwarded).
//! The *crypto* itself is pinned independently by the unit tests
//! in `src/ntlm.rs` against OpenSSL/Python vectors, so a shared
//! bug in both sides cannot hide there. `curl --proxy-ntlm` was
//! also run against the stub by hand during development (see the
//! build log) as an independent client check.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use mock::*;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use fetchline_engine::ntlm::{ntlm_hash, ntlmv2_hash, ntlmv2_response};
use fetchline_engine::secret::{base64_decode, base64_encode};
use fetchline_engine::{DownloadManager, ProxyConfigInput, Store};

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

fn proxy_input(proxy_type: &str, host: &str, port: i64) -> ProxyConfigInput {
    ProxyConfigInput {
        proxy_type: proxy_type.to_string(),
        host: host.to_string(),
        port,
        username: None,
        password: None,
        use_for: vec![],
        bypass: vec![],
        enabled: true,
        auth: None,
    }
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

// ---------------------------------------------------------------------------
// Local FTP fixture server
// ---------------------------------------------------------------------------

/// How the fixture authenticates.
#[derive(Clone)]
enum FtpAuth {
    /// Any credentials accepted (anonymous-style).
    Any,
    /// This exact user/password is required on the origin login.
    Origin(String, String),
    /// Proxy mode: this proxy user/password first, then an origin
    /// login in `user@host` form (any origin credentials accepted).
    Proxy(String, String),
}

struct FtpServer {
    /// `ftp://127.0.0.1:port/file.bin`
    pub url: String,
    pub port: u16,
    /// Every `USER` argument seen, in order — proves the proxy
    /// login sequence (proxy user, then `user@host`).
    pub users: Arc<Mutex<Vec<String>>>,
    /// Control connections opened so far.
    pub connections: Arc<AtomicUsize>,
    /// RETR commands served so far.
    pub retrs: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

struct FtpConfig {
    size_bytes: usize,
    /// When false, SIZE answers 550 — the file is then downloaded
    /// open-ended, as a single segment.
    size_supported: bool,
    /// Sleep between 64 KiB data chunks (lets a pause land mid-download).
    chunk_delay: Duration,
    auth: FtpAuth,
}

impl Default for FtpConfig {
    fn default() -> Self {
        Self {
            size_bytes: 256 * 1024,
            size_supported: true,
            chunk_delay: Duration::ZERO,
            auth: FtpAuth::Any,
        }
    }
}

impl FtpServer {
    fn start(config: FtpConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let users = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let retrs = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let u = Arc::clone(&users);
        let c = Arc::clone(&connections);
        let r = Arc::clone(&retrs);
        let s = Arc::clone(&stop);
        let config = Arc::new(config);
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let u = Arc::clone(&u);
                        let c = Arc::clone(&c);
                        let r = Arc::clone(&r);
                        let cfg = Arc::clone(&config);
                        thread::spawn(move || {
                            c.fetch_add(1, Ordering::SeqCst);
                            handle_ftp(stream, &cfg, u, r);
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            url: format!("ftp://127.0.0.1:{port}/file.bin"),
            port,
            users,
            connections,
            retrs,
            stop,
        }
    }
}

impl Drop for FtpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn ftp_reply(writer: &mut TcpStream, text: &str) {
    let _ = writer.write_all(format!("{text}\r\n").as_bytes());
    let _ = writer.flush();
}

fn handle_ftp(
    stream: TcpStream,
    config: &FtpConfig,
    users: Arc<Mutex<Vec<String>>>,
    retrs: Arc<AtomicUsize>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    ftp_reply(&mut writer, "220 Fetchline test FTP ready");
    // Proxy mode stages: 0 = proxy USER, 1 = proxy PASS, 2 = origin
    // USER (must be user@host), 3 = origin PASS. Direct mode starts
    // at stage 2 (any USER) — or at stage 4 when origin credentials
    // are required (USER must match, then PASS must match).
    let mut stage = match &config.auth {
        FtpAuth::Proxy(_, _) => 0,
        FtpAuth::Origin(_, _) => 4,
        FtpAuth::Any => 2,
    };
    let mut logged_in = false;
    let mut rest: u64 = 0;
    let mut data_listener: Option<TcpListener> = None;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let cmd_line = line.trim_end().to_string();
        let mut parts = cmd_line.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("").to_ascii_uppercase();
        let arg = parts.next().unwrap_or("").to_string();
        match cmd.as_str() {
            "USER" => {
                users.lock().unwrap().push(arg.clone());
                match stage {
                    0 => {
                        // Proxy user.
                        if let FtpAuth::Proxy(user, _) = &config.auth {
                            if arg == *user {
                                stage = 1;
                                ftp_reply(&mut writer, "331 Proxy password required");
                            } else {
                                ftp_reply(&mut writer, "530 Proxy login incorrect");
                            }
                        } else {
                            ftp_reply(&mut writer, "530 Login incorrect");
                        }
                    }
                    1 => {
                        ftp_reply(&mut writer, "331 Send your password");
                        stage = 2;
                    }
                    2 => {
                        // Origin login through the proxy: must be user@host.
                        if matches!(&config.auth, FtpAuth::Proxy(_, _)) && !arg.contains('@') {
                            ftp_reply(&mut writer, "530 Login with user@host");
                        } else {
                            stage = 3;
                            ftp_reply(&mut writer, "331 Password required");
                        }
                    }
                    4 => {
                        if let FtpAuth::Origin(user, _) = &config.auth {
                            if arg == *user {
                                stage = 5;
                                ftp_reply(&mut writer, "331 Password required");
                            } else {
                                ftp_reply(&mut writer, "530 Login incorrect");
                            }
                        } else {
                            ftp_reply(&mut writer, "530 Login incorrect");
                        }
                    }
                    _ => {
                        ftp_reply(&mut writer, "331 Password required");
                        stage = 3;
                    }
                }
            }
            "PASS" => match stage {
                1 => {
                    // Proxy password.
                    if let FtpAuth::Proxy(_, pass) = &config.auth {
                        if arg == *pass {
                            stage = 2;
                            ftp_reply(&mut writer, "230 Proxy login ok");
                        } else {
                            ftp_reply(&mut writer, "530 Proxy login incorrect");
                        }
                    } else {
                        ftp_reply(&mut writer, "530 Login incorrect");
                    }
                }
                3 => {
                    logged_in = true;
                    ftp_reply(&mut writer, "230 Login ok");
                }
                5 => {
                    if let FtpAuth::Origin(_, pass) = &config.auth {
                        if arg == *pass {
                            logged_in = true;
                            ftp_reply(&mut writer, "230 Login ok");
                        } else {
                            ftp_reply(&mut writer, "530 Login incorrect");
                        }
                    } else {
                        ftp_reply(&mut writer, "530 Login incorrect");
                    }
                }
                _ => ftp_reply(&mut writer, "503 Login with USER first"),
            },
            "TYPE" => ftp_reply(&mut writer, "200 Type set to I"),
            "SYST" => ftp_reply(&mut writer, "215 UNIX Type: L8"),
            "FEAT" => ftp_reply(&mut writer, "211 No features"),
            "NOOP" => ftp_reply(&mut writer, "200 OK"),
            "SIZE" => {
                if !logged_in {
                    ftp_reply(&mut writer, "530 Please login first");
                } else if config.size_supported {
                    ftp_reply(&mut writer, &format!("213 {}", config.size_bytes));
                } else {
                    ftp_reply(&mut writer, "550 SIZE not supported");
                }
            }
            "REST" => {
                if !logged_in {
                    ftp_reply(&mut writer, "530 Please login first");
                } else if let Ok(n) = arg.parse::<u64>() {
                    rest = n;
                    ftp_reply(&mut writer, &format!("350 Restarting at {n}"));
                } else {
                    ftp_reply(&mut writer, "501 Bad REST argument");
                }
            }
            "PASV" => {
                if !logged_in {
                    ftp_reply(&mut writer, "530 Please login first");
                } else {
                    match TcpListener::bind("127.0.0.1:0") {
                        Ok(l) => {
                            let p = l.local_addr().unwrap().port();
                            data_listener = Some(l);
                            ftp_reply(
                                &mut writer,
                                &format!(
                                    "227 Entering Passive Mode (127,0,0,1,{},{})",
                                    p / 256,
                                    p % 256
                                ),
                            );
                        }
                        Err(_) => ftp_reply(&mut writer, "425 Cannot open data connection"),
                    }
                }
            }
            "RETR" => {
                if !logged_in {
                    ftp_reply(&mut writer, "530 Please login first");
                    continue;
                }
                let Some(listener) = data_listener.take() else {
                    ftp_reply(&mut writer, "425 Use PASV first");
                    continue;
                };
                retrs.fetch_add(1, Ordering::SeqCst);
                ftp_reply(&mut writer, "150 Opening data connection");
                let Ok((mut data, _)) = listener.accept() else {
                    ftp_reply(&mut writer, "425 Data connection failed");
                    continue;
                };
                let body = payload(config.size_bytes);
                let from = (rest as usize).min(body.len());
                for chunk in body[from..].chunks(65536) {
                    if data.write_all(chunk).is_err() {
                        break;
                    }
                    if config.chunk_delay > Duration::ZERO {
                        thread::sleep(config.chunk_delay);
                    }
                }
                drop(data);
                ftp_reply(&mut writer, "226 Transfer complete");
                rest = 0;
            }
            "QUIT" => {
                ftp_reply(&mut writer, "221 Goodbye");
                return;
            }
            _ => ftp_reply(&mut writer, "500 Unknown command"),
        }
    }
}

// ---------------------------------------------------------------------------
// Local stub HTTP proxy demanding NTLM
// ---------------------------------------------------------------------------

/// A stub HTTP proxy: no auth -> 407 NTLM; Type 1 -> 407 + Type 2;
/// a valid Type 3 -> the request is forwarded to the origin
/// `MockServer` and its response copied back. `negotiate_only`
/// offers only `Negotiate` (Kerberos) instead, for the refusal test.
struct NtlmProxyServer {
    pub port: u16,
    pub saw_type1: Arc<AtomicBool>,
    pub saw_type3: Arc<AtomicBool>,
    /// Requests forwarded to the origin after a valid Type 3.
    pub forwarded: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

const NTLM_SERVER_CHALLENGE: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];

impl NtlmProxyServer {
    fn start(negotiate_only: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let saw_type1 = Arc::new(AtomicBool::new(false));
        let saw_type3 = Arc::new(AtomicBool::new(false));
        let forwarded = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let t1 = Arc::clone(&saw_type1);
        let t3 = Arc::clone(&saw_type3);
        let fwd = Arc::clone(&forwarded);
        let s = Arc::clone(&stop);
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let t1 = Arc::clone(&t1);
                        let t3 = Arc::clone(&t3);
                        let fwd = Arc::clone(&fwd);
                        thread::spawn(move || {
                            handle_ntlm_proxy(stream, negotiate_only, t1, t3, fwd);
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { port, saw_type1, saw_type3, forwarded, stop }
    }
}

impl Drop for NtlmProxyServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Build a Type 2 challenge token (hand-built here, not with the
/// engine's builder — the stub must not share the code under test
/// for the challenge half of the handshake).
fn build_type2_b64() -> String {
    let target = utf16le("DOMAIN");
    // Target info AV pairs: NbDomainName, NbComputerName, Timestamp, Flags, EOL.
    let mut info = Vec::new();
    let av = |id: u16, value: &[u8], out: &mut Vec<u8>| {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(value.len() as u16).to_le_bytes());
        out.extend_from_slice(value);
    };
    av(2, &utf16le("DOMAIN"), &mut info);
    av(1, &utf16le("PROXY"), &mut info);
    av(7, &132223104000000000u64.to_le_bytes(), &mut info);
    av(6, &2u32.to_le_bytes(), &mut info);
    av(0, &[], &mut info);
    let mut msg = Vec::new();
    msg.extend_from_slice(b"NTLMSSP\0");
    msg.extend_from_slice(&2u32.to_le_bytes());
    let name_off = 56u32;
    let info_off = name_off + target.len() as u32;
    msg.extend_from_slice(&(target.len() as u16).to_le_bytes());
    msg.extend_from_slice(&(target.len() as u16).to_le_bytes());
    msg.extend_from_slice(&name_off.to_le_bytes());
    msg.extend_from_slice(&0x00888205u32.to_le_bytes());
    msg.extend_from_slice(&NTLM_SERVER_CHALLENGE);
    msg.extend_from_slice(&0u64.to_le_bytes());
    msg.extend_from_slice(&(info.len() as u16).to_le_bytes());
    msg.extend_from_slice(&(info.len() as u16).to_le_bytes());
    msg.extend_from_slice(&info_off.to_le_bytes());
    msg.extend_from_slice(&[0u8; 8]); // version
    msg.extend_from_slice(&target);
    msg.extend_from_slice(&info);
    base64_encode(&msg)
}

/// Verify a Type 3 against the test password. Recomputes the proof
/// from the Type 3's own blob (client challenge, timestamp, target
/// info) — a wrong password, or a wrong blob layout, fails.
fn verify_type3(bytes: &[u8], password: &str) -> bool {
    if bytes.len() < 64 || &bytes[0..8] != b"NTLMSSP\0" {
        return false;
    }
    if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != 3 {
        return false;
    }
    let rd16 = |o: usize| u16::from_le_bytes(bytes[o..o + 2].try_into().unwrap()) as usize;
    let rd32 = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
    let nt_len = rd16(20);
    let nt_off = rd32(24);
    let dom_len = rd16(28);
    let dom_off = rd32(32);
    let user_len = rd16(36);
    let user_off = rd32(40);
    if nt_off + nt_len > bytes.len() || dom_off + dom_len > bytes.len() || user_off + user_len > bytes.len() {
        return false;
    }
    let domain = String::from_utf16_lossy(
        &bytes[dom_off..dom_off + dom_len]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<u16>>(),
    );
    let user = String::from_utf16_lossy(
        &bytes[user_off..user_off + user_len]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<u16>>(),
    );
    if domain != "DOMAIN" || user != "alice" {
        return false;
    }
    let nt_resp = &bytes[nt_off..nt_off + nt_len];
    if nt_resp.len() < 16 + 32 {
        return false;
    }
    let blob = &nt_resp[16..];
    let timestamp = u64::from_le_bytes(blob[8..16].try_into().unwrap());
    let client_chal: [u8; 8] = blob[16..24].try_into().unwrap();
    let target_info = &blob[28..blob.len() - 4];
    let nt = ntlm_hash(password);
    let v2 = ntlmv2_hash(&nt, &user, &domain);
    let expected = ntlmv2_response(&v2, &NTLM_SERVER_CHALLENGE, &client_chal, timestamp, target_info);
    expected.as_slice() == nt_resp
}

fn read_head(reader: &mut BufReader<TcpStream>) -> Option<(String, Vec<(String, String)>)> {
    let mut first = String::new();
    if reader.read_line(&mut first).ok()? == 0 {
        return None;
    }
    let first = first.trim_end().to_string();
    if first.is_empty() {
        return None;
    }
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end().to_string();
        if line.is_empty() {
            break;
        }
        if let Some((n, v)) = line.split_once(':') {
            headers.push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    Some((first, headers))
}

fn send_407(writer: &mut TcpStream, challenge: &str) {
    let _ = writer.write_all(
        format!(
            "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: {challenge}\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n"
        )
        .as_bytes(),
    );
    let _ = writer.flush();
}

// `authed` is set after a verified Type 3 so a keep-alive client
// can send further requests on the same connection without
// re-authenticating. No client in this harness reuses an
// authenticated connection (each handshake socket is dropped after
// its response), so rustc's unused-assignments lint is correct for
// the harness as it stands — the flag stays because a real proxy
// keeps connections alive, and the allow records why.
#[allow(unused_assignments)]
fn handle_ntlm_proxy(
    stream: TcpStream,
    negotiate_only: bool,
    saw_type1: Arc<AtomicBool>,
    saw_type3: Arc<AtomicBool>,
    forwarded: Arc<AtomicUsize>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut authed = false;
    loop {
        let Some((first, headers)) = read_head(&mut reader) else {
            return;
        };
        let mut parts = first.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let auth = headers
            .iter()
            .find(|(n, _)| n == "proxy-authorization")
            .map(|(_, v)| v.clone());
        if !authed {
            match auth.as_deref() {
                None => {
                    // No credentials at all, or credentials that are
                    // not NTLM (reqwest sends Basic first — a
                    // Basic-only answer never satisfies an
                    // NTLM-only proxy, so it is challenged again
                    // instead of being answered with silence: closing
                    // the connection here made the client fail with
                    // a bare "IncompleteMessage".
                    let challenge = if negotiate_only { "Negotiate" } else { "NTLM" };
                    send_407(&mut writer, challenge);
                    continue;
                }
                Some(value) => {
                    let token = value.split_whitespace().nth(1).unwrap_or("");
                    let bytes = base64_decode(token);
                    match bytes.as_deref() {
                        Some(b) if b.len() >= 12
                            && &b[0..8] == b"NTLMSSP\0"
                            && u32::from_le_bytes(b[8..12].try_into().unwrap()) == 1 =>
                        {
                            saw_type1.store(true, Ordering::SeqCst);
                            let type2 = build_type2_b64();
                            send_407(&mut writer, &format!("NTLM {type2}"));
                            continue;
                        }
                        Some(b) if b.len() >= 12
                            && &b[0..8] == b"NTLMSSP\0"
                            && u32::from_le_bytes(b[8..12].try_into().unwrap()) == 3 =>
                        {
                            saw_type3.store(true, Ordering::SeqCst);
                            // Test-only dummy password for the stub.
                            if verify_type3(b, "hunter2") {
                                authed = true;
                            } else {
                                send_407(&mut writer, "NTLM");
                                continue;
                            }
                        }
                        _ => {
                            let challenge = if negotiate_only { "Negotiate" } else { "NTLM" };
                            send_407(&mut writer, challenge);
                            continue;
                        }
                    }
                }
            }
        }
        // Authenticated (or the request arrived already authed on
        // this connection): forward it to the origin and copy the
        // response back verbatim.
        forwarded.fetch_add(1, Ordering::SeqCst);
        let rest = target.strip_prefix("http://").unwrap_or(&target);
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let Ok(mut upstream) = TcpStream::connect(authority) else {
            let _ = writer.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        };
        let mut out = format!("{method} {path} HTTP/1.1\r\n");
        for (n, v) in &headers {
            if n.starts_with("proxy-") {
                continue;
            }
            out.push_str(&format!("{n}: {v}\r\n"));
        }
        out.push_str("Connection: close\r\n\r\n");
        if upstream.write_all(out.as_bytes()).is_err() {
            return;
        }
        let _ = std::io::copy(&mut upstream, &mut writer);
        return;
    }
}

// ---------------------------------------------------------------------------
// FTP tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ftp_probe_and_download_direct() {
    let server = FtpServer::start(FtpConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let (id, info) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(info.size, Some(256 * 1024));
    assert!(info.accept_ranges);
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.size_bytes, Some(256 * 1024));
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
    assert!(server.retrs.load(Ordering::SeqCst) >= 1, "no RETR was served");
}

#[tokio::test]
async fn ftp_download_multi_segment_byte_exact() {
    // 2 MiB > the 1 MiB segmentation floor: 4 segments, each with
    // its own control + data connection, plus dynamic splits.
    let server = FtpServer::start(FtpConfig {
        size_bytes: 2 * 1024 * 1024,
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(2 * 1024 * 1024));
    assert!(
        server.retrs.load(Ordering::SeqCst) >= 4,
        "expected >= 4 RETRs (one per segment), got {}",
        server.retrs.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn ftp_pause_then_resume_completes_byte_exact() {
    let server = FtpServer::start(FtpConfig {
        size_bytes: 2 * 1024 * 1024,
        chunk_delay: Duration::from_millis(20),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    mgr.pause_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "paused", 10).await);
    let paused = mgr.get_download(&id).unwrap().unwrap();
    assert!(paused.downloaded_bytes > 0, "pause landed before any bytes");
    assert!(paused.downloaded_bytes < 2 * 1024 * 1024, "file finished before the pause");
    mgr.resume_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(2 * 1024 * 1024));
}

#[tokio::test]
async fn ftp_server_requiring_login_accepts_right_credentials_and_refuses_wrong_ones() {
    let server = FtpServer::start(FtpConfig {
        auth: FtpAuth::Origin("alice".to_string(), "hunter2".to_string()),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // Wrong password: the probe fails, nothing is created.
    let bad_url = server.url.replace("ftp://", "ftp://alice:wrongpass@");
    let err = mgr
        .add_download(&bad_url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("530") || err.to_string().contains("refused"), "{err}");
    assert!(mgr.list_downloads().unwrap().is_empty());
    // Right credentials: probe + download succeed.
    let good_url = server.url.replace("ftp://", "ftp://alice:hunter2@");
    let (id, info) = mgr
        .add_download(&good_url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(info.size, Some(256 * 1024));
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
}

#[tokio::test]
async fn ftp_download_without_size_is_open_ended_and_byte_exact() {
    let server = FtpServer::start(FtpConfig {
        size_supported: false,
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let (id, info) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(info.size, None, "SIZE is unsupported, so the probe must not invent a size");
    assert!(!info.accept_ranges);
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
}

#[tokio::test]
async fn ftp_proxy_routes_and_authenticates_in_order() {
    let origin = FtpServer::start(FtpConfig {
        auth: FtpAuth::Proxy("proxyuser".to_string(), "proxypass".to_string()),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("ftp", "127.0.0.1", origin.port as i64);
    input.username = Some("proxyuser".to_string());
    input.password = Some("proxypass".to_string());
    mgr.save_proxy_config(&input).unwrap();
    // The FTP URL points at the origin host (127.0.0.1); the engine
    // must connect to the proxy port instead and log in user@host.
    let (id, _) = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
    let users = origin.users.lock().unwrap().clone();
    assert!(
        users.iter().any(|u| u == "proxyuser"),
        "proxy USER was never sent: {users:?}"
    );
    assert!(
        users.iter().any(|u| u == "anonymous@127.0.0.1"),
        "origin login was not in user@host form: {users:?}"
    );
    assert_eq!(users.first().map(|s| s.as_str()), Some("proxyuser"), "proxy auth must come first: {users:?}");
}

#[tokio::test]
async fn ftp_proxy_wrong_credentials_are_rejected() {
    let origin = FtpServer::start(FtpConfig {
        auth: FtpAuth::Proxy("proxyuser".to_string(), "proxypass".to_string()),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("ftp", "127.0.0.1", origin.port as i64);
    input.username = Some("proxyuser".to_string());
    input.password = Some("wrongpass".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let err = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("530") || err.to_string().contains("refused"),
        "{err}"
    );
    assert!(mgr.list_downloads().unwrap().is_empty());
}

#[tokio::test]
async fn ftp_proxy_bypass_list_goes_direct() {
    // The proxy fixture demands proxy auth; a bypassed host must
    // never talk to it — the direct server (no auth) serves the file.
    let proxy = FtpServer::start(FtpConfig {
        auth: FtpAuth::Proxy("proxyuser".to_string(), "proxypass".to_string()),
        ..Default::default()
    });
    let direct = FtpServer::start(FtpConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("ftp", "127.0.0.1", proxy.port as i64);
    input.username = Some("proxyuser".to_string());
    input.password = Some("proxypass".to_string());
    input.bypass = vec!["127.0.0.1".to_string()];
    mgr.save_proxy_config(&input).unwrap();
    let (id, _) = mgr
        .add_download(&direct.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert_eq!(proxy.connections.load(Ordering::SeqCst), 0, "bypassed host went through the FTP proxy");
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
}

#[tokio::test]
async fn ftp_download_goes_direct_when_only_an_http_proxy_is_configured() {
    // FTP through an HTTP proxy is out of scope: the FTP download
    // goes direct, and the HTTP proxy is never contacted.
    let http_proxy = HttpProxyServer::start();
    let ftp = FtpServer::start(FtpConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.save_proxy_config(&proxy_input("http", "127.0.0.1", http_proxy.port as i64)).unwrap();
    let (id, _) = mgr
        .add_download(&ftp.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert_eq!(http_proxy.hits.load(Ordering::SeqCst), 0, "FTP went through the HTTP proxy");
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
}

#[tokio::test]
async fn test_proxy_ftp_url_through_ftp_proxy_and_http_url_refusals() {
    let origin = FtpServer::start(FtpConfig {
        auth: FtpAuth::Proxy("proxyuser".to_string(), "proxypass".to_string()),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("ftp", "127.0.0.1", origin.port as i64);
    input.username = Some("proxyuser".to_string());
    input.password = Some("proxypass".to_string());
    assert_eq!(mgr.test_proxy(&input, &origin.url).await.unwrap(), 200);
    // An FTP proxy still cannot be tested with an HTTP URL.
    let http = MockServer::start(MockConfig::default());
    let err = mgr.test_proxy(&input, &http.url).await.unwrap_err();
    assert!(err.to_string().contains("FTP"), "{err}");
    // And an HTTP proxy cannot be tested with an FTP URL.
    let http_proxy = proxy_input("http", "127.0.0.1", 1);
    let err = mgr.test_proxy(&http_proxy, &origin.url).await.unwrap_err();
    assert!(err.to_string().contains("FTP"), "{err}");
}

// ---------------------------------------------------------------------------
// NTLM tests
// ---------------------------------------------------------------------------

fn ntlm_proxy_input(port: i64, password: &str) -> ProxyConfigInput {
    let mut input = proxy_input("http", "127.0.0.1", port);
    // Test-only dummy credentials: DOMAIN\alice / hunter2.
    input.username = Some("DOMAIN\\alice".to_string());
    input.password = Some(password.to_string());
    input
}

#[tokio::test]
async fn ntlm_proxy_authenticates_http_download() {
    let origin = MockServer::start(MockConfig::default());
    let proxy = NtlmProxyServer::start(false);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.save_proxy_config(&ntlm_proxy_input(proxy.port as i64, "hunter2")).unwrap();
    let (id, info) = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(info.size, Some(512 * 1024));
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    assert!(proxy.saw_type1.load(Ordering::SeqCst), "no NTLM Type 1 was sent");
    assert!(proxy.saw_type3.load(Ordering::SeqCst), "no NTLM Type 3 was sent");
    assert!(proxy.forwarded.load(Ordering::SeqCst) > 0, "nothing was forwarded after auth");
}

#[tokio::test]
async fn ntlm_proxy_wrong_password_is_rejected() {
    let origin = MockServer::start(MockConfig::default());
    let proxy = NtlmProxyServer::start(false);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.save_proxy_config(&ntlm_proxy_input(proxy.port as i64, "wrongpass")).unwrap();
    let err = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("407"), "{err}");
    assert!(mgr.list_downloads().unwrap().is_empty());
    assert!(proxy.saw_type3.load(Ordering::SeqCst), "the engine never sent a Type 3 to be refused");
}

#[tokio::test]
async fn ntlm_proxy_test_button_succeeds_and_wrong_password_fails() {
    let origin = MockServer::start(MockConfig::default());
    let proxy = NtlmProxyServer::start(false);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let good = ntlm_proxy_input(proxy.port as i64, "hunter2");
    assert_eq!(mgr.test_proxy(&good, &origin.url).await.unwrap(), 200);
    assert!(proxy.saw_type1.load(Ordering::SeqCst), "test never did the NTLM handshake");
    assert!(proxy.forwarded.load(Ordering::SeqCst) > 0, "test never reached the origin");
    let bad = ntlm_proxy_input(proxy.port as i64, "wrongpass");
    let err = mgr.test_proxy(&bad, &origin.url).await.unwrap_err();
    assert!(err.to_string().contains("407"), "{err}");
}

#[tokio::test]
async fn kerberos_only_proxy_is_refused_clearly() {
    let origin = MockServer::start(MockConfig::default());
    let proxy = NtlmProxyServer::start(true);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.save_proxy_config(&ntlm_proxy_input(proxy.port as i64, "hunter2")).unwrap();
    let err = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Kerberos"), "{err}");
    let input = ntlm_proxy_input(proxy.port as i64, "hunter2");
    let err = mgr.test_proxy(&input, &origin.url).await.unwrap_err();
    assert!(err.to_string().contains("Kerberos"), "{err}");
}

#[tokio::test]
async fn ntlm_proxy_https_test_url_is_refused_clearly() {
    let proxy = NtlmProxyServer::start(false);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let input = ntlm_proxy_input(proxy.port as i64, "hunter2");
    let err = mgr
        .test_proxy(&input, "https://127.0.0.1:1/file.bin")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("NTLM") && err.to_string().contains("https"),
        "{err}"
    );
}
