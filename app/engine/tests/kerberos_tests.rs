//! Kerberos (SPNEGO / `Negotiate`) proxy authentication.
//!
//! Every test serves its own fixture on 127.0.0.1 — a local stub
//! HTTP proxy that demands `Negotiate` (`NegotiateProxyServer`
//! below) and the shared `MockServer` origin. No public server is
//! ever used, no KDC is ever contacted, and no password here is
//! real: all credentials are test-only dummies (`alice` /
//! `hunter2`, `DOMAIN\alice` / `hunter2`), never logged and never
//! stored outside the test.
//!
//! What the stub proves, and what it does not prove:
//! - The stub validates the SPNEGO framing with its OWN
//!   hand-written DER code (it does NOT share the engine's
//!   encoder/decoder), and builds its `NegTokenResp` with its own
//!   encoder. A framing bug in the engine cannot hide behind the
//!   same code on both sides; the engine's encoder is pinned
//!   independently by the unit tests in `src/kerberos.rs` against
//!   hand-computed vectors.
//! - The end-to-end success path uses a scripted token source
//!   (`ScriptedInitiator`) through the
//!   `kerberos_request_with_initiator` seam: it proves the
//!   framing, the multi-round token exchange, and the forwarding
//!   — NOT a real domain login. A real Kerberos context needs a
//!   KDC, which is out of scope (no KDC exists on this VM).
//! - The failure path (no Kerberos credentials on this VM) goes
//!   through the REAL GSSAPI initiator and the REAL manager/probe
//!   paths: it proves the engine attempts Kerberos, fails with a
//!   clear error naming Kerberos, and falls back (or refuses) in
//!   the documented order — never silently downgrading to Basic.

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
use fetchline_engine::{
    build_neg_token_init, build_neg_token_resp, kerberos_request_with_initiator,
    DownloadManager, EngineError, ProxyConfig, ProxyConfigInput, SpnegoInitiator, SpnegoStep,
    Store,
};

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
// Scripted token source (test double for the GSSAPI/SSPI initiator)
// ---------------------------------------------------------------------------

/// A `SpnegoInitiator` that returns scripted tokens. It records the
/// input token of every step, so a test can prove the engine fed
/// the acceptor's response token into the next step. `fail_at` is
/// the 0-based step that fails (simulating "no Kerberos
/// credentials"), instead of returning a token.
struct ScriptedInitiator {
    tokens: Vec<Vec<u8>>,
    fail_at: Option<usize>,
    calls: Arc<AtomicUsize>,
    /// The input token each step received, in order.
    inputs: Arc<Mutex<Vec<Option<Vec<u8>>>>>,
}

impl ScriptedInitiator {
    fn new(tokens: Vec<Vec<u8>>) -> Self {
        Self { tokens, fail_at: None, calls: Arc::new(AtomicUsize::new(0)), inputs: Arc::new(Mutex::new(Vec::new())) }
    }

    fn failing() -> Self {
        let mut s = Self::new(vec![]);
        s.fail_at = Some(0);
        s
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn inputs(&self) -> Vec<Option<Vec<u8>>> {
        self.inputs.lock().unwrap().clone()
    }
}

impl SpnegoInitiator for ScriptedInitiator {
    fn step(&mut self, input_token: Option<&[u8]>) -> Result<SpnegoStep, EngineError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs.lock().unwrap().push(input_token.map(|t| t.to_vec()));
        if self.fail_at == Some(n) {
            return Err(EngineError::InvalidState(
                "Kerberos (Negotiate) proxy authentication failed: simulated test failure — no credentials in the test cache".to_string(),
            ));
        }
        let token = self.tokens.get(n).cloned().unwrap_or_default();
        Ok(SpnegoStep { token, complete: n + 1 >= self.tokens.len() })
    }
}

/// Read a whole raw-proxy body through the returned body reader.
async fn read_all_body(mut reader: fetchline_engine::ntlm::NtlmBodyReader) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read_chunk(&mut buf).await.unwrap();
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    out
}

// ---------------------------------------------------------------------------
// Independent SPNEGO DER helpers for the stub (do NOT share the
// engine's encoder/decoder — see the file docs).
// ---------------------------------------------------------------------------

fn stub_der_len(len: usize) -> Vec<u8> {
    if len < 128 {
        vec![len as u8]
    } else if len < 256 {
        vec![0x81, len as u8]
    } else {
        vec![0x82, (len >> 8) as u8, len as u8]
    }
}

fn stub_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend_from_slice(&stub_der_len(content.len()));
    out.extend_from_slice(content);
    out
}

/// The stub's own `NegTokenResp` encoder: `A1 <len> 30 <len> A0
/// <len> 0A 01 <state> [A2 <len> 04 <len> <token>]`.
fn stub_neg_token_resp(neg_state: u8, response_token: Option<&[u8]>) -> Vec<u8> {
    let mut inner = stub_tlv(0xa0, &stub_tlv(0x0a, &[neg_state]));
    if let Some(token) = response_token {
        inner.extend_from_slice(&stub_tlv(0xa2, &stub_tlv(0x04, token)));
    }
    stub_tlv(0xa1, &stub_tlv(0x30, &inner))
}

/// Read one TLV: `(tag, content, next_offset)`. Independent of the
/// engine's reader.
fn stub_read_tlv(bytes: &[u8], offset: usize) -> Option<(u8, &[u8], usize)> {
    if offset + 2 > bytes.len() {
        return None;
    }
    let tag = bytes[offset];
    let first = bytes[offset + 1];
    let (len, mut pos) = if first & 0x80 == 0 {
        (first as usize, offset + 2)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || offset + 2 + n > bytes.len() {
            return None;
        }
        let mut len = 0usize;
        for b in &bytes[offset + 2..offset + 2 + n] {
            len = (len << 8) | *b as usize;
        }
        (len, offset + 2 + n)
    };
    if pos + len > bytes.len() {
        return None;
    }
    let content = &bytes[pos..pos + len];
    pos += len;
    Some((tag, content, pos))
}

/// Validate a `NegTokenInit` the engine sent: it must be a `0x60`
/// token whose OID is SPNEGO (1.3.6.1.5.5.2) and whose mech list
/// contains the Kerberos OID — and return its mech token.
fn stub_validate_neg_token_init(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.is_empty() || bytes[0] != 0x60 {
        return None;
    }
    let (_, content, _) = stub_read_tlv(bytes, 0)?;
    let (tag, oid, pos) = stub_read_tlv(content, 0)?;
    if tag != 0x06 || oid != [0x2b, 0x06, 0x01, 0x05, 0x05, 0x02] {
        return None;
    }
    let (tag, init_seq, _) = stub_read_tlv(content, pos)?;
    if tag != 0xa0 {
        return None;
    }
    let (tag, fields, _) = stub_read_tlv(init_seq, 0)?;
    if tag != 0x30 {
        return None;
    }
    let mut saw_kerberos_oid = false;
    let mut mech_token = None;
    let mut p = 0;
    while let Some((tag, c, next)) = stub_read_tlv(fields, p) {
        match tag {
            0xa0 => {
                let (_, list, _) = stub_read_tlv(c, 0)?;
                let mut q = 0;
                while let Some((t, o, n)) = stub_read_tlv(list, q) {
                    if t == 0x06 && o == [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 0x01, 0x02, 0x02] {
                        saw_kerberos_oid = true;
                    }
                    q = n;
                }
            }
            0xa2 => {
                let (_, token, _) = stub_read_tlv(c, 0)?;
                mech_token = Some(token.to_vec());
            }
            _ => {}
        }
        p = next;
    }
    if saw_kerberos_oid {
        mech_token
    } else {
        None
    }
}

/// Validate a `NegTokenResp` the engine sent (a later-round token):
/// it must be a `0xA1` token carrying a response token.
fn stub_validate_neg_token_resp(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.is_empty() || bytes[0] != 0xa1 {
        return None;
    }
    let (_, content, _) = stub_read_tlv(bytes, 0)?;
    let (_, fields, _) = stub_read_tlv(content, 0)?;
    let mut p = 0;
    while let Some((tag, c, next)) = stub_read_tlv(fields, p) {
        if tag == 0xa2 {
            let (_, token, _) = stub_read_tlv(c, 0)?;
            return Some(token.to_vec());
        }
        p = next;
    }
    None
}

// ---------------------------------------------------------------------------
// Local stub HTTP proxy demanding Negotiate (and, optionally, NTLM/Basic)
// ---------------------------------------------------------------------------

/// What the stub offers on its 407s.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StubOffer {
    NegotiateOnly,
    NegotiateAndBasic,
    NegotiateAndNtlm,
    BasicOnly,
}

struct NegotiateProxyServer {
    pub port: u16,
    /// A well-formed `NegTokenInit` was received.
    pub saw_negotiate_init: Arc<AtomicBool>,
    /// A well-formed `NegTokenResp` (second round) was received.
    pub saw_negotiate_resp: Arc<AtomicBool>,
    /// An NTLM Type 1 / Type 3 was received.
    pub saw_type1: Arc<AtomicBool>,
    pub saw_type3: Arc<AtomicBool>,
    /// Requests forwarded to the origin after authentication.
    pub forwarded: Arc<AtomicUsize>,
    /// Every `Proxy-Authorization` header value seen, in order —
    /// proves what the engine did (and did not) send.
    pub auth_headers: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

const NTLM_SERVER_CHALLENGE: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];

impl NegotiateProxyServer {
    fn start(offer: StubOffer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let saw_negotiate_init = Arc::new(AtomicBool::new(false));
        let saw_negotiate_resp = Arc::new(AtomicBool::new(false));
        let saw_type1 = Arc::new(AtomicBool::new(false));
        let saw_type3 = Arc::new(AtomicBool::new(false));
        let forwarded = Arc::new(AtomicUsize::new(0));
        let auth_headers = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (a, b, c, d, e, f, s) = (
            Arc::clone(&saw_negotiate_init),
            Arc::clone(&saw_negotiate_resp),
            Arc::clone(&saw_type1),
            Arc::clone(&saw_type3),
            Arc::clone(&forwarded),
            Arc::clone(&auth_headers),
            Arc::clone(&stop),
        );
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let (a, b, c, d, e, f) = (
                            Arc::clone(&a),
                            Arc::clone(&b),
                            Arc::clone(&c),
                            Arc::clone(&d),
                            Arc::clone(&e),
                            Arc::clone(&f),
                        );
                        thread::spawn(move || {
                            handle_negotiate_proxy(stream, offer, a, b, c, d, e, f);
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            port,
            saw_negotiate_init,
            saw_negotiate_resp,
            saw_type1,
            saw_type3,
            forwarded,
            auth_headers,
            stop,
        }
    }

    /// Did the engine ever send a `Proxy-Authorization: Basic …` header?
    fn saw_basic(&self) -> bool {
        self.auth_headers
            .lock()
            .unwrap()
            .iter()
            .any(|h| h.to_ascii_lowercase().starts_with("basic "))
    }

    /// Did the engine ever send a `Proxy-Authorization: Negotiate …` header?
    fn saw_negotiate(&self) -> bool {
        self.auth_headers
            .lock()
            .unwrap()
            .iter()
            .any(|h| h.to_ascii_lowercase().starts_with("negotiate "))
    }
}

impl Drop for NegotiateProxyServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Build an NTLM Type 2 challenge token (hand-built here, as in
/// `proxy_tests.rs` — the stub must not share the engine's builder
/// for the challenge half of the handshake).
fn build_type2_b64() -> String {
    let target = utf16le("DOMAIN");
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

/// Verify an NTLM Type 3 against the test password (same approach
/// as `proxy_tests.rs`: recompute the proof from the Type 3's own
/// blob). Test-only dummy password: `hunter2`.
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

fn send_407(writer: &mut TcpStream, challenges: &[String]) {
    let mut out = String::from("HTTP/1.1 407 Proxy Authentication Required\r\n");
    for c in challenges {
        out.push_str(&format!("Proxy-Authenticate: {c}\r\n"));
    }
    out.push_str("Content-Length: 0\r\nConnection: keep-alive\r\n\r\n");
    let _ = writer.write_all(out.as_bytes());
    let _ = writer.flush();
}

fn offer_challenges(offer: StubOffer) -> Vec<String> {
    match offer {
        StubOffer::NegotiateOnly => vec!["Negotiate".to_string()],
        StubOffer::NegotiateAndBasic => {
            vec!["Negotiate".to_string(), "Basic realm=\"proxy\"".to_string()]
        }
        StubOffer::NegotiateAndNtlm => vec!["Negotiate".to_string(), "NTLM".to_string()],
        StubOffer::BasicOnly => vec!["Basic realm=\"proxy\"".to_string()],
    }
}

#[allow(unused_assignments)]
fn handle_negotiate_proxy(
    stream: TcpStream,
    offer: StubOffer,
    saw_negotiate_init: Arc<AtomicBool>,
    saw_negotiate_resp: Arc<AtomicBool>,
    saw_type1: Arc<AtomicBool>,
    saw_type3: Arc<AtomicBool>,
    forwarded: Arc<AtomicUsize>,
    auth_headers: Arc<Mutex<Vec<String>>>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut authed = false;
    // Test-only dummy Basic credentials for this stub.
    let expected_basic = basic_auth_value("alice", "hunter2");
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
        if let Some(value) = &auth {
            auth_headers.lock().unwrap().push(value.clone());
        }
        if !authed {
            match auth.as_deref() {
                None => {
                    send_407(&mut writer, &offer_challenges(offer));
                    continue;
                }
                Some(value) => {
                    let scheme = value.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
                    let token = value.split_whitespace().nth(1).unwrap_or("");
                    match scheme.as_str() {
                        "negotiate" => {
                            let bytes = base64_decode(token).unwrap_or_default();
                            if stub_validate_neg_token_init(&bytes).is_some() {
                                saw_negotiate_init.store(true, Ordering::SeqCst);
                                // One continuation round: accept-incomplete
                                // with a response token, then the next
                                // token completes the handshake.
                                let resp = stub_neg_token_resp(1, Some(b"stub-response-token"));
                                send_407(
                                    &mut writer,
                                    &[format!("Negotiate {}", base64_encode(&resp))],
                                );
                                continue;
                            }
                            if stub_validate_neg_token_resp(&bytes).is_some() {
                                saw_negotiate_resp.store(true, Ordering::SeqCst);
                                authed = true;
                            } else {
                                send_407(&mut writer, &offer_challenges(offer));
                                continue;
                            }
                        }
                        "ntlm" => {
                            let bytes = base64_decode(token).unwrap_or_default();
                            match bytes.as_slice() {
                                b if b.len() >= 12
                                    && &b[0..8] == b"NTLMSSP\0"
                                    && u32::from_le_bytes(b[8..12].try_into().unwrap()) == 1 =>
                                {
                                    saw_type1.store(true, Ordering::SeqCst);
                                    let type2 = build_type2_b64();
                                    send_407(&mut writer, &[format!("NTLM {type2}")]);
                                    continue;
                                }
                                b if b.len() >= 12
                                    && &b[0..8] == b"NTLMSSP\0"
                                    && u32::from_le_bytes(b[8..12].try_into().unwrap()) == 3 =>
                                {
                                    saw_type3.store(true, Ordering::SeqCst);
                                    if verify_type3(b, "hunter2") {
                                        authed = true;
                                    } else {
                                        send_407(&mut writer, &["NTLM".to_string()]);
                                        continue;
                                    }
                                }
                                _ => {
                                    send_407(&mut writer, &offer_challenges(offer));
                                    continue;
                                }
                            }
                        }
                        "basic" => {
                            if value == expected_basic {
                                authed = true;
                            } else {
                                send_407(&mut writer, &offer_challenges(offer));
                                continue;
                            }
                        }
                        _ => {
                            send_407(&mut writer, &offer_challenges(offer));
                            continue;
                        }
                    }
                }
            }
        }
        // Authenticated: forward it to the origin and copy the
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

fn kerberos_proxy_config(port: u16) -> ProxyConfig {
    ProxyConfig {
        proxy_type: "http".to_string(),
        host: "127.0.0.1".to_string(),
        port: port as i64,
        username: None,
        password: None,
        use_for: vec![],
        bypass: vec![],
        auth: "negotiate-auto".to_string(),
    }
}

// ---------------------------------------------------------------------------
// The SPNEGO exchange itself (scripted initiator + stub)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kerberos_proxy_authenticates_http_request() {
    let origin = MockServer::start(MockConfig::default());
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateOnly);
    let cfg = kerberos_proxy_config(proxy.port);
    // Two scripted steps: NegTokenInit, then NegTokenResp.
    let init = build_neg_token_init(b"mech-token-1");
    let cont = build_neg_token_resp(0, Some(b"mech-token-2"));
    let initiator = ScriptedInitiator::new(vec![init, cont]);
    let calls = Arc::clone(&initiator.calls);
    let inputs = Arc::clone(&initiator.inputs);
    let (status, _headers, reader) =
        kerberos_request_with_initiator(&cfg, "GET", &origin.url, None, Box::new(initiator), None)
            .await
            .unwrap();
    assert_eq!(status, 200);
    assert_eq!(read_all_body(reader).await, payload(512 * 1024));
    assert_eq!(calls.load(Ordering::SeqCst), 2, "expected exactly two SPNEGO steps");
    // The stub's response token was fed into the second step.
    let seen_inputs = inputs.lock().unwrap().clone();
    assert_eq!(seen_inputs.len(), 2);
    assert_eq!(seen_inputs[0], None, "the first step takes no input token");
    assert_eq!(seen_inputs[1], Some(b"stub-response-token".to_vec()));
    assert!(proxy.saw_negotiate_init.load(Ordering::SeqCst), "no well-formed NegTokenInit reached the stub");
    assert!(proxy.saw_negotiate_resp.load(Ordering::SeqCst), "no well-formed NegTokenResp reached the stub");
    assert_eq!(proxy.forwarded.load(Ordering::SeqCst), 1, "nothing was forwarded after the handshake");
    assert!(!proxy.saw_basic(), "Basic auth was sent on a Negotiate-only proxy");
}

#[tokio::test]
async fn kerberos_proxy_ranged_get_and_head_work() {
    let origin = MockServer::start(MockConfig::default());
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateOnly);
    let cfg = kerberos_proxy_config(proxy.port);
    // Ranged GET: the handshake, then a 206 with the first 1 KiB.
    let initiator = ScriptedInitiator::new(vec![
        build_neg_token_init(b"mech-token-1"),
        build_neg_token_resp(0, Some(b"mech-token-2")),
    ]);
    let (status, headers, reader) = kerberos_request_with_initiator(
        &cfg,
        "GET",
        &origin.url,
        Some((0, Some(1023))),
        Box::new(initiator),
        None,
    )
    .await
    .unwrap();
    assert_eq!(status, 206);
    let body = read_all_body(reader).await;
    assert_eq!(body.len(), 1024);
    assert_eq!(body, payload(512 * 1024)[..1024]);
    assert!(
        headers.iter().any(|(n, v)| n == "content-range" && v == "bytes 0-1023/524288"),
        "no Content-Range on the ranged reply: {headers:?}"
    );
    // HEAD through the same exchange.
    let initiator = ScriptedInitiator::new(vec![
        build_neg_token_init(b"mech-token-1"),
        build_neg_token_resp(0, Some(b"mech-token-2")),
    ]);
    let (status, headers, reader) =
        kerberos_request_with_initiator(&cfg, "HEAD", &origin.url, None, Box::new(initiator), None)
            .await
            .unwrap();
    assert_eq!(status, 200);
    assert_eq!(read_all_body(reader).await.len(), 0, "a HEAD must have no body");
    assert!(
        headers.iter().any(|(n, v)| n == "content-length" && v == "524288"),
        "no Content-Length on the HEAD reply: {headers:?}"
    );
}

#[tokio::test]
async fn kerberos_proxy_initial_407_token_reaches_the_first_step() {
    // When the 407 that triggered the handshake already carries a
    // token, it is the first step's input — not dropped.
    let origin = MockServer::start(MockConfig::default());
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateOnly);
    let cfg = kerberos_proxy_config(proxy.port);
    let initiator = ScriptedInitiator::new(vec![
        build_neg_token_init(b"mech-token-1"),
        build_neg_token_resp(0, Some(b"mech-token-2")),
    ]);
    let inputs = Arc::clone(&initiator.inputs);
    let (status, _, reader) = kerberos_request_with_initiator(
        &cfg,
        "GET",
        &origin.url,
        None,
        Box::new(initiator),
        Some(b"initial-stub-token".to_vec()),
    )
    .await
    .unwrap();
    assert_eq!(status, 200);
    assert_eq!(read_all_body(reader).await, payload(512 * 1024));
    let seen_inputs = inputs.lock().unwrap().clone();
    assert_eq!(seen_inputs[0], Some(b"initial-stub-token".to_vec()));
}

#[tokio::test]
async fn kerberos_proxy_failing_initiator_is_a_clear_kerberos_error() {
    let origin = MockServer::start(MockConfig::default());
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateOnly);
    let cfg = kerberos_proxy_config(proxy.port);
    let err = kerberos_request_with_initiator(
        &cfg,
        "GET",
        &origin.url,
        None,
        Box::new(ScriptedInitiator::failing()),
        None,
    )
    .await
    .err()
    .expect("a failing initiator must error, not return a response");
    assert!(err.to_string().contains("Kerberos"), "{err}");
    assert!(!proxy.saw_basic(), "a failed Kerberos attempt sent Basic auth");
    assert!(!proxy.saw_negotiate(), "a failed Kerberos attempt sent a Negotiate token");
    assert_eq!(proxy.forwarded.load(Ordering::SeqCst), 0, "a failed handshake forwarded a request");
}

// ---------------------------------------------------------------------------
// The real GSSAPI initiator through the real manager/probe paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kerberos_only_proxy_fails_clearly_with_real_gssapi_and_never_sends_basic() {
    // No Kerberos credentials exist on this VM (no ticket cache, no
    // krb5.conf), so the REAL GSSAPI initiator fails — and the
    // failure must name Kerberos and never fall through to Basic.
    let origin = MockServer::start(MockConfig::default());
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateOnly);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("DOMAIN\\alice".to_string());
    input.password = Some("hunter2".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let err = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Kerberos"), "{err}");
    assert!(mgr.list_downloads().unwrap().is_empty());
    assert!(!proxy.saw_basic(), "the engine fell through to Basic on a Negotiate-only proxy");
    assert!(!proxy.saw_negotiate(), "a token was sent without Kerberos credentials");
    // The Test button reports the same failure.
    let err = mgr.test_proxy(&input, &origin.url).await.unwrap_err();
    assert!(err.to_string().contains("Kerberos"), "{err}");
}

#[tokio::test]
async fn kerberos_falls_back_to_basic_when_no_ticket() {
    // The stub offers Negotiate AND Basic; the real GSSAPI
    // initiator fails (no ticket), so the download must complete
    // over Basic — Kerberos tried first, Basic second.
    let origin = MockServer::start(MockConfig::default());
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateAndBasic);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("alice".to_string());
    input.password = Some("hunter2".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let (id, info) = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(info.size, Some(512 * 1024));
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    assert!(proxy.saw_basic(), "the fallback never sent Basic auth");
    assert!(proxy.forwarded.load(Ordering::SeqCst) > 0, "nothing was forwarded after the fallback");
    // The Test button takes the same fallback.
    assert_eq!(mgr.test_proxy(&input, &origin.url).await.unwrap(), 200);
}

#[tokio::test]
async fn kerberos_falls_back_to_ntlm_when_no_ticket() {
    // The stub offers Negotiate AND NTLM; the real GSSAPI
    // initiator fails (no ticket), so the download must complete
    // over NTLM — Kerberos tried first, NTLM second.
    let origin = MockServer::start(MockConfig::default());
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateAndNtlm);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("DOMAIN\\alice".to_string());
    input.password = Some("hunter2".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let (id, info) = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(info.size, Some(512 * 1024));
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    assert!(proxy.saw_type1.load(Ordering::SeqCst), "no NTLM Type 1 was sent on the fallback");
    assert!(proxy.saw_type3.load(Ordering::SeqCst), "no NTLM Type 3 was sent on the fallback");
    assert!(proxy.forwarded.load(Ordering::SeqCst) > 0, "nothing was forwarded after the fallback");
    assert_eq!(mgr.test_proxy(&input, &origin.url).await.unwrap(), 200);
}

#[tokio::test]
async fn kerberos_auth_setting_never_downgrades_to_basic() {
    // auth=kerberos with a proxy that offers only Basic: the
    // engine must refuse with a clear Kerberos error, never send
    // Basic credentials it was told not to use.
    let origin = MockServer::start(MockConfig::default());
    let proxy = NegotiateProxyServer::start(StubOffer::BasicOnly);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("alice".to_string());
    input.password = Some("hunter2".to_string());
    input.auth = Some("kerberos".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let err = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Kerberos"), "{err}");
    assert!(mgr.list_downloads().unwrap().is_empty());
    assert!(!proxy.saw_basic(), "auth=kerberos sent Basic credentials to a Basic-only proxy");
    let err = mgr.test_proxy(&input, &origin.url).await.unwrap_err();
    assert!(err.to_string().contains("Kerberos"), "{err}");
}

#[tokio::test]
async fn ntlm_auth_setting_never_uses_kerberos() {
    // auth=ntlm with a proxy that offers only Negotiate: the
    // engine must refuse with a clear NTLM error, never attempt
    // Kerberos it was told not to use.
    let origin = MockServer::start(MockConfig::default());
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateOnly);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("DOMAIN\\alice".to_string());
    input.password = Some("hunter2".to_string());
    input.auth = Some("ntlm".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let err = mgr
        .add_download(&origin.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("NTLM"), "{err}");
    assert!(mgr.list_downloads().unwrap().is_empty());
    assert!(!proxy.saw_negotiate(), "auth=ntlm sent a Negotiate token");
    assert!(!proxy.saw_type1.load(Ordering::SeqCst), "auth=ntlm sent an NTLM Type 1 to a Negotiate-only proxy");
}

#[tokio::test]
async fn kerberos_https_test_url_is_refused_clearly() {
    // Kerberos through an https:// proxy needs a TLS tunnel after
    // the CONNECT handshake, which is not built — same limit as NTLM.
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateOnly);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("DOMAIN\\alice".to_string());
    input.password = Some("hunter2".to_string());
    input.auth = Some("kerberos".to_string());
    let err = mgr
        .test_proxy(&input, "https://127.0.0.1:1/file.bin")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Kerberos") && err.to_string().contains("https"),
        "{err}"
    );
}

// ---------------------------------------------------------------------------
// Auth setting: storage, validation, migration
// ---------------------------------------------------------------------------

#[test]
fn proxy_auth_setting_round_trip_and_validation() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    // New proxy defaults to negotiate-auto.
    let row = store.save_proxy_config(&proxy_input("http", "127.0.0.1", 8080)).unwrap();
    assert_eq!(row.auth, "negotiate-auto");
    assert_eq!(store.get_active_proxy().unwrap().unwrap().auth, "negotiate-auto");
    // Explicit Kerberos sticks, and saving without auth keeps it.
    let mut input = proxy_input("http", "127.0.0.1", 8080);
    input.auth = Some("kerberos".to_string());
    let row = store.save_proxy_config(&input).unwrap();
    assert_eq!(row.auth, "kerberos");
    let row = store.save_proxy_config(&proxy_input("http", "127.0.0.1", 8080)).unwrap();
    assert_eq!(row.auth, "kerberos", "saving without auth must keep the stored setting");
    // Switching to Basic sticks too.
    let mut input = proxy_input("http", "127.0.0.1", 8080);
    input.auth = Some("basic".to_string());
    let row = store.save_proxy_config(&input).unwrap();
    assert_eq!(row.auth, "basic");
    // Unknown auth values and Kerberos on a SOCKS proxy are refused.
    let mut bad = proxy_input("http", "127.0.0.1", 8080);
    bad.auth = Some("digest".to_string());
    assert!(store.save_proxy_config(&bad).is_err());
    let mut bad = proxy_input("socks", "127.0.0.1", 8080);
    bad.auth = Some("kerberos".to_string());
    assert!(store.save_proxy_config(&bad).is_err());
    let mut bad = proxy_input("ftp", "127.0.0.1", 8080);
    bad.auth = Some("ntlm".to_string());
    assert!(store.save_proxy_config(&bad).is_err());
    // The rejected saves stored nothing new.
    assert_eq!(store.get_proxy_config().unwrap().unwrap().auth, "basic");
}

#[test]
fn proxy_auth_column_migrates_on_old_databases() {
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        // A database from before Kerberos: the proxy table has no
        // `auth` column at all.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1),
             default_download_folder TEXT NOT NULL DEFAULT '',
             max_connections INTEGER NOT NULL DEFAULT 8,
             speed_limit_kbps INTEGER, language TEXT NOT NULL DEFAULT 'en',
             theme TEXT NOT NULL DEFAULT 'light',
             browser_integration INTEGER NOT NULL DEFAULT 1,
             antivirus_command TEXT, file_types_auto TEXT NOT NULL DEFAULT '[]',
             excluded_sites TEXT NOT NULL DEFAULT '[]',
             check_updates_weekly INTEGER NOT NULL DEFAULT 1,
             updated_at TEXT NOT NULL DEFAULT (datetime('now')));
             INSERT INTO settings (id) VALUES (1);
             CREATE TABLE proxy_configs (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE,
             type TEXT NOT NULL DEFAULT 'http', host TEXT NOT NULL,
             port INTEGER NOT NULL, username TEXT, password_enc TEXT,
             use_for TEXT NOT NULL DEFAULT '[]', enabled INTEGER NOT NULL DEFAULT 0,
             bypass TEXT NOT NULL DEFAULT '[]',
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')));",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let row = store.save_proxy_config(&proxy_input("http", "127.0.0.1", 8080)).unwrap();
    assert_eq!(row.auth, "negotiate-auto", "a pre-Kerberos row migrates to the default");
    assert_eq!(store.get_active_proxy().unwrap().unwrap().auth, "negotiate-auto");
}

// ---------------------------------------------------------------------------
// Cookie-gated download behind a proxy (the probe_with_cookies path)
// ---------------------------------------------------------------------------

/// A tiny origin that serves one file only with the right cookie,
/// with minimal Range support. Test-only, not the shared mock.
struct CookieGatedOrigin {
    pub url: String,
    stop: Arc<AtomicBool>,
}

impl CookieGatedOrigin {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        thread::spawn(move || handle_gated(stream));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { url: format!("http://127.0.0.1:{port}/file.bin"), stop }
    }
}

impl Drop for CookieGatedOrigin {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn handle_gated(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let Some((first, headers)) = read_head(&mut reader) else {
        return;
    };
    let method = first.split_whitespace().next().unwrap_or("").to_string();
    let has_cookie = headers
        .iter()
        .any(|(n, v)| n == "cookie" && v.contains("session=abc"));
    let body = payload(128 * 1024);
    if !has_cookie {
        let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    if method == "HEAD" {
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        );
        return;
    }
    let range = headers
        .iter()
        .find(|(n, _)| n == "range")
        .map(|(_, v)| v.clone());
    if let Some(range) = range {
        // `bytes=<start>-<end>` only.
        let spec = range.trim_start_matches("bytes=");
        let (start, end) = match spec.split_once('-') {
            Some((a, b)) => (
                a.parse::<usize>().unwrap_or(0),
                b.parse::<usize>().unwrap_or(body.len() - 1),
            ),
            None => (0, body.len() - 1),
        };
        let end = end.min(body.len() - 1);
        let chunk = &body[start..=end];
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                chunk.len(),
                start,
                end,
                body.len()
            )
            .as_bytes(),
        );
        let _ = stream.write_all(chunk);
        return;
    }
    let _ = stream.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    );
    let _ = stream.write_all(&body);
}

#[tokio::test]
async fn cookie_gated_download_behind_basic_proxy_still_works() {
    // Regression guard for the auth change: with the default
    // `negotiate-auto` setting the shared client sends no
    // preemptive Basic auth, so the cookie probe must answer the
    // proxy's 407 with Basic (cookies attached) instead of failing.
    let origin = CookieGatedOrigin::start();
    let proxy = HttpProxyServer::start_with_auth(Some(("alice", "hunter2")));
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("alice".to_string());
    input.password = Some("hunter2".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let (id, info) = mgr
        .add_download_with_cookies(
            &origin.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            Some("session=abc".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(info.size, Some(128 * 1024));
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(128 * 1024));
    assert!(proxy.hits.load(Ordering::SeqCst) > 0, "the proxy was never used");
}

#[tokio::test]
async fn cookie_gated_download_behind_kerberos_only_proxy_fails_clearly() {
    // The raw Kerberos handshake cannot carry browser cookies, so
    // a cookie-gated link behind a Negotiate-only proxy fails with
    // a clear error naming Kerberos — not a bare 407.
    let origin = CookieGatedOrigin::start();
    let proxy = NegotiateProxyServer::start(StubOffer::NegotiateOnly);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("DOMAIN\\alice".to_string());
    input.password = Some("hunter2".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let err = mgr
        .add_download_with_cookies(
            &origin.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            Some("session=abc".to_string()),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Kerberos"), "{err}");
    assert!(mgr.list_downloads().unwrap().is_empty());
}
