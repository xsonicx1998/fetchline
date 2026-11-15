//! NTLMv2 proxy authentication (MS-NLMP) for HTTP proxies.
//!
//! What is supported, honestly:
//! - NTLMv2 over plain HTTP through an HTTP proxy: the engine speaks
//!   the Type 1 / Type 2 / Type 3 handshake on a raw TCP connection to
//!   the proxy and then performs the request on that same, now
//!   authenticated, connection (NTLM is connection-bound, so the
//!   handshake and the request must share one socket — reqwest cannot
//!   do this, which is why this module bypasses it for NTLM).
//!   The handshake is auto-negotiated: a proxy that answers 407 with
//!   `Proxy-Authenticate: NTLM` triggers it; a proxy that answers
//!   Basic keeps using reqwest's Basic auth. No new setting is needed
//!   — the proxy username may be `DOMAIN\user` or `user@domain` (the
//!   domain is split off) or a bare username.
//! - Kerberos / SPNEGO (`Negotiate`) is supported too — see
//!   `kerberos.rs`. A proxy that demands only Negotiate is handled
//!   by the Kerberos path (or refused with a clear Kerberos error
//!   when no ticket is available), never by this module.
//! - HTTPS through an NTLM proxy is NOT supported: the CONNECT tunnel
//!   would need a TLS handshake on the authenticated socket, which
//!   this module does not implement. The error says so plainly.
//! - Signing / sealing (NTLM message integrity / encryption) is NOT
//!   implemented: proxy authentication only needs the Type 3
//!   response, and no request body is ever sealed.
//!
//! The raw-HTTP helpers at the bottom (`read_response_head`,
//! `drain_body`, `header_value`, `NtlmBodyReader`) are shared with
//! the Kerberos path (`kerberos.rs`), which speaks the same shape
//! of raw exchange — they are `pub(crate)` for exactly that reason.
//!
//! Crypto: MD4, MD5 and HMAC-MD5 are implemented here from their
//! public specifications (RFC 1320 / RFC 1321 / RFC 2104) with no new
//! dependency, so the engine builds offline. They are pinned by unit
//! tests against independent vectors (OpenSSL `md4`, Python
//! `hashlib.md5`, and an independently computed NTLMv2 response —
//! see `tests/proxy_tests.rs` and the vector in
//! `/tmp/ntlm_vector.py`, kept out of the repo because it is a
//! scratch oracle, not a fixture). The NTLM hash of "Password" is
//! the published `a4f49c406510bdcab6824ee7c30fd852`.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::{EngineError, Result};
use crate::proxy::ProxyConfig;
use crate::secret::{base64_decode, base64_encode};

// ---------------------------------------------------------------------------
// MD5 (RFC 1321)
// ---------------------------------------------------------------------------

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
    0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
    0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
    0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
    0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
    0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
    0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
    0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
    0xeb86d391,
];

/// MD5 digest of `data`.
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (mut f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            f = f
                .wrapping_add(a)
                .wrapping_add(MD5_K[i])
                .wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(MD5_S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------
// MD4 (RFC 1320)
// ---------------------------------------------------------------------------

fn md4_round_f(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (!x & z)
}
fn md4_round_g(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (x & z) | (y & z)
}
fn md4_round_h(x: u32, y: u32, z: u32) -> u32 {
    x ^ y ^ z
}

/// MD4 digest of `data`.
pub fn md4(data: &[u8]) -> [u8; 16] {
    let mut a: u32 = 0x67452301;
    let mut b: u32 = 0xefcdab89;
    let mut c: u32 = 0x98badcfe;
    let mut d: u32 = 0x10325476;
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut x = [0u32; 16];
        for (i, w) in x.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let (aa, bb, cc, dd) = (a, b, c, d);
        // Round 1: F, shifts 3,7,11,19.
        let s1 = [3u32, 7, 11, 19];
        for i in 0..16 {
            let (a2, b2, c2, d2) = match i % 4 {
                0 => {
                    a = a
                        .wrapping_add(md4_round_f(b, c, d))
                        .wrapping_add(x[i])
                        .rotate_left(s1[i % 4]);
                    (a, b, c, d)
                }
                1 => {
                    d = d
                        .wrapping_add(md4_round_f(a, b, c))
                        .wrapping_add(x[i])
                        .rotate_left(s1[i % 4]);
                    (a, b, c, d)
                }
                2 => {
                    c = c
                        .wrapping_add(md4_round_f(d, a, b))
                        .wrapping_add(x[i])
                        .rotate_left(s1[i % 4]);
                    (a, b, c, d)
                }
                _ => {
                    b = b
                        .wrapping_add(md4_round_f(c, d, a))
                        .wrapping_add(x[i])
                        .rotate_left(s1[i % 4]);
                    (a, b, c, d)
                }
            };
            a = a2;
            b = b2;
            c = c2;
            d = d2;
        }
        // Round 2: G + 0x5a827999, shifts 3,5,9,13, order 0,4,8,12,1,5,9,13,2,6,10,14,3,7,11,15.
        let s2 = [3u32, 5, 9, 13];
        for i in 0..16 {
            let k = [0usize, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15][i];
            match i % 4 {
                0 => {
                    a = a
                        .wrapping_add(md4_round_g(b, c, d))
                        .wrapping_add(x[k])
                        .wrapping_add(0x5a827999)
                        .rotate_left(s2[i % 4]);
                }
                1 => {
                    d = d
                        .wrapping_add(md4_round_g(a, b, c))
                        .wrapping_add(x[k])
                        .wrapping_add(0x5a827999)
                        .rotate_left(s2[i % 4]);
                }
                2 => {
                    c = c
                        .wrapping_add(md4_round_g(d, a, b))
                        .wrapping_add(x[k])
                        .wrapping_add(0x5a827999)
                        .rotate_left(s2[i % 4]);
                }
                _ => {
                    b = b
                        .wrapping_add(md4_round_g(c, d, a))
                        .wrapping_add(x[k])
                        .wrapping_add(0x5a827999)
                        .rotate_left(s2[i % 4]);
                }
            }
        }
        // Round 3: H + 0x6ed9eba1, shifts 3,9,11,15, order 0,8,4,12,2,10,6,14,1,9,5,13,3,11,7,15.
        let s3 = [3u32, 9, 11, 15];
        for i in 0..16 {
            let k = [0usize, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15][i];
            match i % 4 {
                0 => {
                    a = a
                        .wrapping_add(md4_round_h(b, c, d))
                        .wrapping_add(x[k])
                        .wrapping_add(0x6ed9eba1)
                        .rotate_left(s3[i % 4]);
                }
                1 => {
                    d = d
                        .wrapping_add(md4_round_h(a, b, c))
                        .wrapping_add(x[k])
                        .wrapping_add(0x6ed9eba1)
                        .rotate_left(s3[i % 4]);
                }
                _ if i % 4 == 2 => {
                    c = c
                        .wrapping_add(md4_round_h(d, a, b))
                        .wrapping_add(x[k])
                        .wrapping_add(0x6ed9eba1)
                        .rotate_left(s3[i % 4]);
                }
                _ => {
                    b = b
                        .wrapping_add(md4_round_h(c, d, a))
                        .wrapping_add(x[k])
                        .wrapping_add(0x6ed9eba1)
                        .rotate_left(s3[i % 4]);
                }
            }
        }
        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a.to_le_bytes());
    out[4..8].copy_from_slice(&b.to_le_bytes());
    out[8..12].copy_from_slice(&c.to_le_bytes());
    out[12..16].copy_from_slice(&d.to_le_bytes());
    out
}

/// HMAC-MD5 (RFC 2104) with the MD5 above.
pub fn hmac_md5(key: &[u8], msg: &[u8]) -> [u8; 16] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let hashed = md5(key);
        k[..16].copy_from_slice(&hashed);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + msg.len());
    inner.extend(k.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(msg);
    let inner_hash = md5(&inner);
    let mut outer = Vec::with_capacity(64 + 16);
    outer.extend(k.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&inner_hash);
    md5(&outer)
}

// ---------------------------------------------------------------------------
// NTLMv2 message construction (MS-NLMP)
// ---------------------------------------------------------------------------

/// UTF-16LE encoding of a string, as NTLM uses everywhere.
pub fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

/// Split a proxy username into `(domain, user)`: `DOMAIN\user`,
/// `user@domain`, or a bare `user` (empty domain).
pub fn split_user(raw: &str) -> (String, String) {
    if let Some((domain, user)) = raw.split_once('\\') {
        return (domain.to_string(), user.to_string());
    }
    if let Some((user, domain)) = raw.split_once('@') {
        if !user.is_empty() && !domain.is_empty() {
            return (domain.to_string(), user.to_string());
        }
    }
    (String::new(), raw.to_string())
}

/// The NT hash: MD4 of the UTF-16LE password.
pub fn ntlm_hash(password: &str) -> [u8; 16] {
    md4(&utf16le(password))
}

/// The NTLMv2 hash: HMAC-MD5 of the NT hash over
/// UTF-16LE(UPPERCASE(username) + domain).
pub fn ntlmv2_hash(nt_hash: &[u8; 16], user: &str, domain: &str) -> [u8; 16] {
    let mut msg = utf16le(&user.to_uppercase());
    msg.extend_from_slice(&utf16le(domain));
    hmac_md5(nt_hash, &msg)
}

/// NTLM flags used for Type 1 and Type 3: Unicode, request target,
/// NTLM, always sign, NTLM2 key, target info.
const NTLM_FLAGS: u32 = 0x0088_8205;

/// Build a Type 1 (negotiate) message.
pub fn build_type1(domain: &str, workstation: &str) -> Vec<u8> {
    let mut msg = Vec::with_capacity(64);
    msg.extend_from_slice(b"NTLMSSP\0");
    msg.extend_from_slice(&1u32.to_le_bytes());
    msg.extend_from_slice(&NTLM_FLAGS.to_le_bytes());
    let domain_b = utf16le(domain);
    let ws_b = utf16le(workstation);
    let mut offset = 32u32;
    msg.extend_from_slice(&(domain_b.len() as u16).to_le_bytes());
    msg.extend_from_slice(&(domain_b.len() as u16).to_le_bytes());
    msg.extend_from_slice(&offset.to_le_bytes());
    offset += domain_b.len() as u32;
    msg.extend_from_slice(&(ws_b.len() as u16).to_le_bytes());
    msg.extend_from_slice(&(ws_b.len() as u16).to_le_bytes());
    msg.extend_from_slice(&offset.to_le_bytes());
    msg.extend_from_slice(&domain_b);
    msg.extend_from_slice(&ws_b);
    msg
}

/// A parsed Type 2 (challenge) message.
#[derive(Debug, Clone)]
pub struct NtlmChallenge {
    pub server_challenge: [u8; 8],
    /// The target name the server sent (often the domain).
    pub target_name: String,
    /// The raw target-info AV pairs, copied into the Type 3 blob.
    pub target_info: Vec<u8>,
    /// The timestamp AV pair inside target info, when present.
    pub target_timestamp: Option<u64>,
    pub flags: u32,
}

/// Parse a base64 Type 2 token (with or without the `NTLM ` prefix).
pub fn parse_type2(token: &str) -> Result<NtlmChallenge> {
    let token = token.strip_prefix("NTLM ").unwrap_or(token).trim();
    let bytes = base64_decode(token)
        .ok_or_else(|| EngineError::InvalidState("The proxy sent a bad NTLM challenge".to_string()))?;
    if bytes.len() < 48
        || &bytes[0..8] != b"NTLMSSP\0"
        || u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != 2
    {
        return Err(EngineError::InvalidState(
            "The proxy sent a bad NTLM challenge".to_string(),
        ));
    }
    let rd16 = |o: usize| u16::from_le_bytes(bytes[o..o + 2].try_into().unwrap()) as usize;
    let rd32 = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let target_len = rd16(12);
    let target_off = rd32(16) as usize;
    let flags = rd32(20);
    let mut server_challenge = [0u8; 8];
    server_challenge.copy_from_slice(&bytes[24..32]);
    let target_name = if target_off + target_len <= bytes.len() {
        let raw = &bytes[target_off..target_off + target_len];
        if flags & 0x1 != 0 {
            String::from_utf16_lossy(
                &raw.chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect::<Vec<u16>>(),
            )
        } else {
            String::from_utf8_lossy(raw).into_owned()
        }
    } else {
        String::new()
    };
    let info_len = rd16(40);
    let info_off = rd32(44) as usize;
    let target_info = if info_off + info_len <= bytes.len() {
        bytes[info_off..info_off + info_len].to_vec()
    } else {
        Vec::new()
    };
    // The timestamp AV pair (id 7) inside target info, when present.
    let mut target_timestamp = None;
    let mut i = 0;
    while i + 4 <= target_info.len() {
        let id = u16::from_le_bytes(target_info[i..i + 2].try_into().unwrap());
        let len = u16::from_le_bytes(target_info[i + 2..i + 4].try_into().unwrap()) as usize;
        if id == 0 || i + 4 + len > target_info.len() {
            break;
        }
        if id == 7 && len == 8 {
            target_timestamp = Some(u64::from_le_bytes(
                target_info[i + 4..i + 12].try_into().unwrap(),
            ));
        }
        i += 4 + len;
    }
    Ok(NtlmChallenge {
        server_challenge,
        target_name,
        target_info,
        target_timestamp,
        flags,
    })
}

/// Current time as a Windows FILETIME (100 ns ticks since 1601-01-01).
pub fn filetime_now() -> u64 {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    unix.as_secs() * 10_000_000 + unix.subsec_nanos() as u64 / 100 + 11644473600 * 10_000_000
}

/// An 8-byte client challenge. Uniqueness, not secrecy, is what
/// matters; it mixes the clock, the process id and a counter.
pub fn client_challenge() -> [u8; 8] {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut mixed = now.as_nanos() as u64 ^ ((std::process::id() as u64) << 32) ^ n.wrapping_mul(0x9e3779b97f4a7c15);
    let mut out = [0u8; 8];
    for b in out.iter_mut() {
        mixed ^= mixed >> 33;
        mixed = mixed.wrapping_mul(0xff51afd7ed558ccd);
        mixed ^= mixed >> 33;
        *b = mixed as u8;
    }
    out
}

/// The NTLMv2 response (proof + blob) for fixed inputs — the unit
/// test pins this against the independently computed vector.
pub fn ntlmv2_response(
    ntlm_v2_hash: &[u8; 16],
    server_challenge: &[u8; 8],
    client_challenge: &[u8; 8],
    timestamp: u64,
    target_info: &[u8],
) -> Vec<u8> {
    let mut blob = Vec::with_capacity(32 + target_info.len() + 4);
    blob.extend_from_slice(&[0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    blob.extend_from_slice(&timestamp.to_le_bytes());
    blob.extend_from_slice(client_challenge);
    blob.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    blob.extend_from_slice(target_info);
    blob.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    let mut msg = Vec::with_capacity(8 + blob.len());
    msg.extend_from_slice(server_challenge);
    msg.extend_from_slice(&blob);
    let proof = hmac_md5(ntlm_v2_hash, &msg);
    let mut resp = Vec::with_capacity(16 + blob.len());
    resp.extend_from_slice(&proof);
    resp.extend_from_slice(&blob);
    resp
}

/// The LMv2 response: HMAC-MD5 over server + client challenge,
/// followed by the client challenge.
pub fn lmv2_response(
    ntlm_v2_hash: &[u8; 16],
    server_challenge: &[u8; 8],
    client_challenge: &[u8; 8],
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(16);
    msg.extend_from_slice(server_challenge);
    msg.extend_from_slice(client_challenge);
    let proof = hmac_md5(ntlm_v2_hash, &msg);
    let mut resp = Vec::with_capacity(24);
    resp.extend_from_slice(&proof);
    resp.extend_from_slice(client_challenge);
    resp
}

/// Build a Type 3 (authenticate) message with fixed challenge /
/// timestamp (the unit test uses this; production uses
/// `build_type3_now`).
#[allow(clippy::too_many_arguments)]
pub fn build_type3(
    user: &str,
    domain: &str,
    workstation: &str,
    password: &str,
    challenge: &NtlmChallenge,
    client_chal: &[u8; 8],
    timestamp: u64,
) -> Vec<u8> {
    let nt_hash = ntlm_hash(password);
    let v2_hash = ntlmv2_hash(&nt_hash, user, domain);
    let nt_resp = ntlmv2_response(&v2_hash, &challenge.server_challenge, client_chal, timestamp, &challenge.target_info);
    let lm_resp = lmv2_response(&v2_hash, &challenge.server_challenge, client_chal);
    let domain_b = utf16le(domain);
    let user_b = utf16le(user);
    let ws_b = utf16le(workstation);
    let mut msg = Vec::with_capacity(64 + lm_resp.len() + nt_resp.len());
    msg.extend_from_slice(b"NTLMSSP\0");
    msg.extend_from_slice(&3u32.to_le_bytes());
    let mut offset = 64u32;
    let field = |msg: &mut Vec<u8>, data: &[u8], offset: &mut u32| {
        msg.extend_from_slice(&(data.len() as u16).to_le_bytes());
        msg.extend_from_slice(&(data.len() as u16).to_le_bytes());
        msg.extend_from_slice(&offset.to_le_bytes());
        *offset += data.len() as u32;
    };
    field(&mut msg, &lm_resp, &mut offset);
    field(&mut msg, &nt_resp, &mut offset);
    field(&mut msg, &domain_b, &mut offset);
    field(&mut msg, &user_b, &mut offset);
    field(&mut msg, &ws_b, &mut offset);
    // Empty session key.
    msg.extend_from_slice(&0u16.to_le_bytes());
    msg.extend_from_slice(&0u16.to_le_bytes());
    msg.extend_from_slice(&offset.to_le_bytes());
    msg.extend_from_slice(&NTLM_FLAGS.to_le_bytes());
    msg.extend_from_slice(&lm_resp);
    msg.extend_from_slice(&nt_resp);
    msg.extend_from_slice(&domain_b);
    msg.extend_from_slice(&user_b);
    msg.extend_from_slice(&ws_b);
    msg
}

/// Build a Type 3 with a fresh client challenge and timestamp (the
/// server's timestamp AV pair is preferred when the challenge
/// carried one — it is the server's own clock).
pub fn build_type3_now(
    user: &str,
    domain: &str,
    workstation: &str,
    password: &str,
    challenge: &NtlmChallenge,
) -> Vec<u8> {
    let timestamp = challenge.target_timestamp.unwrap_or_else(filetime_now);
    build_type3(user, domain, workstation, password, challenge, &client_challenge(), timestamp)
}

// ---------------------------------------------------------------------------
// Raw HTTP through an NTLM proxy
// ---------------------------------------------------------------------------

const NTLM_TIMEOUT: Duration = Duration::from_secs(30);

/// One HTTP response header, as received (name lowercased).
pub type RawHeader = (String, String);

/// Read one HTTP response head (status line + headers) from a buffered
/// stream. Returns `(status, headers)`. Shared with `kerberos.rs`.
pub(crate) async fn read_response_head(
    reader: &mut BufReader<TcpStream>,
) -> Result<(u16, Vec<RawHeader>)> {
    let mut status_line = String::new();
    reader.read_line(&mut status_line).await?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            EngineError::InvalidState(format!(
                "The proxy answered with a bad response: {status_line}"
            ))
        })?;
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let line = line.trim_end().to_string();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
        if headers.len() > 200 {
            return Err(EngineError::InvalidState(
                "The proxy sent too many headers".to_string(),
            ));
        }
    }
    Ok((status, headers))
}

pub(crate) fn header_value<'a>(headers: &'a [RawHeader], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Discard a response body (Content-Length only — a 407 body is
/// never chunked in practice) so the next request on the same
/// connection starts clean. Shared with `kerberos.rs`.
pub(crate) async fn drain_body(reader: &mut BufReader<TcpStream>, headers: &[RawHeader]) -> Result<()> {
    if let Some(len) = header_value(headers, "content-length").and_then(|v| v.parse::<u64>().ok()) {
        let mut left = len;
        let mut buf = [0u8; 4096];
        while left > 0 {
            let want = (left as usize).min(buf.len());
            let n = reader.read(&mut buf[..want]).await?;
            if n == 0 {
                break;
            }
            left -= n as u64;
        }
    }
    Ok(())
}

fn proxy_authenticate_ntlm(headers: &[RawHeader]) -> Option<String> {
    headers.iter().find_map(|(n, v)| {
        if n == "proxy-authenticate" && v.to_ascii_lowercase().contains("ntlm") {
            Some(v.clone())
        } else {
            None
        }
    })
}

fn proxy_authenticate_negotiate_only(headers: &[RawHeader]) -> bool {    let mut saw_negotiate = false;
    let mut saw_ntlm = false;
    for (n, v) in headers {
        if n == "proxy-authenticate" {
            let lower = v.to_ascii_lowercase();
            if lower.contains("negotiate") {
                saw_negotiate = true;
            }
            if lower.contains("ntlm") {
                saw_ntlm = true;
            }
        }
    }
    saw_negotiate && !saw_ntlm
}

/// The body of an NTLM-proxied response, read in chunks. `read_chunk`
/// handles Content-Length, chunked transfer encoding, and
/// read-to-close bodies, so the manager's segment pump can read it
/// exactly like any other body without loading a whole segment into
/// memory.
pub struct NtlmBodyReader {
    inner: BufReader<TcpStream>,
    /// Bytes left under Content-Length, when the length is known.
    remaining: Option<u64>,
    /// Chunked transfer encoding in progress.
    chunked: bool,
    chunk_remaining: u64,
    done: bool,
}

impl NtlmBodyReader {
    /// Shared with `kerberos.rs`: the body reader is not
    /// NTLM-specific, it just decodes Content-Length / chunked /
    /// read-to-close bodies off a raw socket.
    pub(crate) fn new(inner: BufReader<TcpStream>, headers: &[RawHeader]) -> Self {
        let chunked = header_value(headers, "transfer-encoding")
            .map(|v| v.to_ascii_lowercase().contains("chunked"))
            .unwrap_or(false);
        let remaining = if chunked {
            None
        } else {
            header_value(headers, "content-length").and_then(|v| v.parse().ok())
        };
        Self {
            inner,
            remaining,
            chunked,
            chunk_remaining: 0,
            done: false,
        }
    }

    /// Read up to `buf.len()` decoded body bytes. Returns 0 at EOF.
    pub async fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.chunked {
            if self.chunk_remaining == 0 {
                // Next chunk-size line (after a previous chunk's CRLF).
                let mut line = String::new();
                self.inner.read_line(&mut line).await?;
                let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16)
                    .unwrap_or(0);
                if size == 0 {
                    // Trailers, then the final CRLF.
                    loop {
                        let mut t = String::new();
                        self.inner.read_line(&mut t).await?;
                        if t.trim().is_empty() {
                            break;
                        }
                    }
                    self.done = true;
                    return Ok(0);
                }
                self.chunk_remaining = size as u64;
            }
            let want = (self.chunk_remaining as usize).min(buf.len());
            let n = self.inner.read(&mut buf[..want]).await?;
            if n == 0 {
                self.done = true;
                return Ok(0);
            }
            self.chunk_remaining -= n as u64;
            if self.chunk_remaining == 0 {
                // The CRLF after the chunk.
                let mut crlf = [0u8; 2];
                let _ = self.inner.read_exact(&mut crlf).await;
            }
            return Ok(n);
        }
        // Content-Length or read-to-close.
        let limit = match self.remaining {
            Some(r) => (r as usize).min(buf.len()),
            None => buf.len(),
        };
        if limit == 0 {
            self.done = true;
            return Ok(0);
        }
        let n = self.inner.read(&mut buf[..limit]).await?;
        if n == 0 {
            self.done = true;
            return Ok(0);
        }
        if let Some(r) = &mut self.remaining {
            *r -= n as u64;
            if *r == 0 {
                self.done = true;
            }
        }
        Ok(n)
    }
}

/// Perform one HTTP request through an HTTP proxy that demands NTLM
/// auth, on a single connection: Type 1, read the Type 2 challenge
/// from the 407, then the real request with Type 3 on the same
/// socket. Returns `(status, headers, body reader)`; the caller
/// reads the body (or drops it) and must not reuse the socket.
/// `range` is `Some((start, end))` for a ranged GET, `None` for a
/// plain request. Only plain `http://` targets are supported — see
/// the module docs for why https is refused.
pub async fn ntlm_request(
    proxy: &ProxyConfig,
    method: &str,
    url: &str,
    range: Option<(u64, Option<u64>)>,
) -> Result<(u16, Vec<RawHeader>, NtlmBodyReader)> {
    let parsed = reqwest::Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
    if parsed.scheme() != "http" {
        return Err(EngineError::InvalidState(
            "NTLM proxy authentication is only supported for http:// URLs — \
             https:// through an NTLM proxy needs a TLS tunnel after the \
             handshake, which is not built"
                .to_string(),
        ));
    }
    let (username, password) = match (&proxy.username, &proxy.password) {
        (Some(u), Some(p)) if !u.trim().is_empty() => (u.clone(), p.clone()),
        _ => {
            return Err(EngineError::InvalidState(
                "The proxy needs NTLM authentication — set the proxy username and password".to_string(),
            ))
        }
    };
    let (domain, user) = split_user(&username);
    let host = parsed.host_str().unwrap_or("").to_string();
    let port = parsed.port_or_known_default().unwrap_or(80);
    let target = format!("http://{host}:{port}{}", {
        let mut t = parsed.path().to_string();
        if let Some(q) = parsed.query() {
            t.push('?');
            t.push_str(q);
        }
        t
    });
    let host_header = if port == 80 { host.clone() } else { format!("{host}:{port}") };
    let range_header = range.map(|(start, end)| match end {
        Some(e) => format!("bytes={start}-{e}"),
        None => format!("bytes={start}-"),
    });
    let request_line = |auth: &str| {
        let mut r = format!(
            "{method} {target} HTTP/1.1\r\nHost: {host_header}\r\nProxy-Authorization: {auth}\r\nProxy-Connection: Keep-Alive\r\nConnection: close\r\n"
        );
        if let Some(rh) = &range_header {
            r.push_str(&format!("Range: {rh}\r\n"));
        }
        r.push_str("\r\n");
        r
    };

    let work = async {
        let stream = TcpStream::connect((proxy.host.as_str(), proxy.port as u16)).await?;
        let mut reader = BufReader::new(stream);
        // Type 1.
        let type1 = base64_encode(&build_type1(&domain, "FETCHLINE"));
        reader
            .write_all(request_line(&format!("NTLM {type1}")).as_bytes())
            .await?;
        reader.flush().await?;
        let (status, headers) = read_response_head(&mut reader).await?;
        if status != 407 {
            // The proxy accepted the Type 1 (or needs no auth after
            // all): the response to the real request is already here.
            let body = NtlmBodyReader::new(reader, &headers);
            return Ok::<_, EngineError>((status, headers, body));
        }
        if proxy_authenticate_negotiate_only(&headers) {
            // This socket was opened for NTLM, but the proxy only
            // offers Negotiate — the caller picked the wrong scheme
            // (the fallback order in `proxy.rs` routes Negotiate to
            // the Kerberos path first, so this is a misconfiguration
            // or a proxy that changed its mind mid-handshake).
            return Err(EngineError::InvalidState(
                "This proxy needs Kerberos (Negotiate) authentication, not NTLM — \
                 set the proxy authentication to Kerberos or Negotiate (auto)".to_string(),
            ));
        }
        // Drain the 407 body (if any) before sending the next
        // request on this connection, or its bytes would be read
        // as the start of the next response head.
        drain_body(&mut reader, &headers).await?;
        let challenge_token = proxy_authenticate_ntlm(&headers).ok_or_else(|| {
            EngineError::InvalidState(
                "Proxy authentication failed (407) — check the proxy username and password".to_string(),
            )
        })?;
        // The Type 2 token is the base64 after "NTLM ".
        let token = challenge_token
            .split_whitespace()
            .nth(1)
            .unwrap_or("")
            .to_string();
        let challenge = parse_type2(&token)?;
        // Type 3 + the real request, same connection.
        let type3 = base64_encode(&build_type3_now(&user, &domain, "FETCHLINE", &password, &challenge));
        reader
            .write_all(request_line(&format!("NTLM {type3}")).as_bytes())
            .await?;
        reader.flush().await?;
        let (status, headers) = read_response_head(&mut reader).await?;
        let body = NtlmBodyReader::new(reader, &headers);
        Ok::<_, EngineError>((status, headers, body))
    };
    tokio::time::timeout(NTLM_TIMEOUT, work)
        .await
        .map_err(|_| {
            EngineError::InvalidState("The proxy did not answer the NTLM handshake in time".to_string())
        })?
}

/// What a proxy demands for a CONNECT tunnel, learned by sending
/// one unauthenticated CONNECT and reading its 407. `None` (the
/// function returns `None`) means "could not tell" — the caller
/// proceeds with the normal request and lets it fail on its own.
#[derive(Debug, PartialEq, Eq)]
pub enum ConnectAuthKind {
    /// Basic auth (or anything reqwest already handles).
    Basic,
    /// NTLM — reqwest cannot do this for a tunnel.
    Ntlm,
    /// `Negotiate` only — Kerberos, which is not supported.
    NegotiateOnly,
    /// The tunnel opened without auth.
    NoAuth,
}

/// Send one unauthenticated CONNECT for an `https://` URL and read
/// what the proxy demands. Used only to *explain* a failed CONNECT
/// (reqwest surfaces a proxy 407 to CONNECT as a transport error,
/// not as a 407 response, so the clear error would otherwise be
/// lost). Never sends credentials.
pub async fn probe_connect_auth(proxy: &ProxyConfig, url: &str) -> Option<ConnectAuthKind> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);
    let work = async {
        let stream = TcpStream::connect((proxy.host.as_str(), proxy.port as u16)).await.ok()?;
        let mut reader = BufReader::new(stream);
        reader
            .write_all(format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n").as_bytes())
            .await
            .ok()?;
        reader.flush().await.ok()?;
        let (status, headers) = read_response_head(&mut reader).await.ok()?;
        if status == 200 {
            return Some(ConnectAuthKind::NoAuth);
        }
        if status != 407 {
            return Some(ConnectAuthKind::Basic);
        }
        if proxy_authenticate_negotiate_only(&headers) {
            return Some(ConnectAuthKind::NegotiateOnly);
        }
        if proxy_authenticate_ntlm(&headers).is_some() {
            return Some(ConnectAuthKind::Ntlm);
        }
        Some(ConnectAuthKind::Basic)
    };
    tokio::time::timeout(NTLM_TIMEOUT, work).await.ok().flatten()
}

/// HEAD a URL through an NTLM proxy and return `(status, headers)`.
/// The (empty) body is dropped.
pub async fn ntlm_head(proxy: &ProxyConfig, url: &str) -> Result<(u16, Vec<RawHeader>)> {
    let (status, headers, mut body) = ntlm_request(proxy, "HEAD", url, None).await?;
    // Drain nothing: a HEAD has no body by definition.
    let _ = &mut body;
    Ok((status, headers))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_known_vectors() {
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(&md5(b"The quick brown fox jumps over the lazy dog")),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        // Multi-block (56+ bytes) + a 1,000,000-byte input.
        assert_eq!(hex(&md5(&vec![b'a'; 1_000_000])), "7707d6ae4e027c70eea2a935c2296f21");
    }

    #[test]
    fn md4_known_vectors() {
        assert_eq!(hex(&md4(b"")), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(hex(&md4(b"a")), "bde52cb31de33e46245e05fbdbd6fb24");
        assert_eq!(hex(&md4(b"abc")), "a448017aaf21d8525fc10ae87aa6729d");
        assert_eq!(hex(&md4(b"The quick brown fox jumps over the lazy dog")), "1bee69a46ba811185c194762abaeae90");
    }

    #[test]
    fn hmac_md5_known_vector() {
        // RFC 2104 test case 2: key "Jefe", data "what do ya want for nothing?"
        assert_eq!(
            hex(&hmac_md5(b"Jefe", b"what do ya want for nothing?")),
            "750c783e6ab0b503eaa86e310a5db738"
        );
    }

    #[test]
    fn ntlm_hash_of_password_matches_published_value() {
        assert_eq!(hex(&ntlm_hash("Password")), "a4f49c406510bdcab6824ee7c30fd852");
    }

    #[test]
    fn ntlmv2_response_matches_independent_vector() {
        // Vector computed independently in /tmp/ntlm_vector.py
        // (OpenSSL MD4 + Python hashlib/hmac): user "User", domain
        // "Domain", password "Password", server challenge
        // 0123456789abcdef, client challenge aaaa… (8 bytes),
        // timestamp 132223104000000000 (2020-01-01 UTC), target
        // info NbComputerName "Server" + NbDomainName "Domain" +
        // Timestamp + Flags.
        let nt = ntlm_hash("Password");
        assert_eq!(
            hex(&ntlmv2_hash(&nt, "User", "Domain")),
            "0c868a403bfd7a93a3001ef22ef02e3f"
        );
        let target_info =
            hex_decode("01000c0053006500720076006500720002000c0044006f006d00610069006e00070008000000056936c0d501060004000200000000000000");
        let resp = ntlmv2_response(
            &ntlmv2_hash(&nt, "User", "Domain"),
            &hex8("0123456789abcdef"),
            &hex8("aaaaaaaaaaaaaaaa"),
            132223104000000000,
            &target_info,
        );
        assert_eq!(
            hex(&resp),
            "388dd49db4190534b32d6d8378b8edbf01010000000000000000056936c0d501aaaaaaaaaaaaaaaa0000000001000c0053006500720076006500720002000c0044006f006d00610069006e00070008000000056936c0d50106000400020000000000000000000000"
        );
        let lm = lmv2_response(
            &ntlmv2_hash(&nt, "User", "Domain"),
            &hex8("0123456789abcdef"),
            &hex8("aaaaaaaaaaaaaaaa"),
        );
        assert_eq!(hex(&lm), "86c35097ac9cec102554764a57cccc19aaaaaaaaaaaaaaaa");
    }

    #[test]
    fn type1_type2_type3_round_trip() {
        let type1 = build_type1("DOMAIN", "FETCHLINE");
        assert_eq!(&type1[0..8], b"NTLMSSP\0");
        assert_eq!(u32::from_le_bytes(type1[8..12].try_into().unwrap()), 1);
        // A hand-built Type 2 with a known challenge parses back.
        let mut type2 = Vec::new();
        type2.extend_from_slice(b"NTLMSSP\0");
        type2.extend_from_slice(&2u32.to_le_bytes());
        let target = utf16le("DOMAIN");
        type2.extend_from_slice(&(target.len() as u16).to_le_bytes());
        type2.extend_from_slice(&(target.len() as u16).to_le_bytes());
        type2.extend_from_slice(&48u32.to_le_bytes());
        type2.extend_from_slice(&0x00888205u32.to_le_bytes());
        type2.extend_from_slice(&hex8("0123456789abcdef"));
        type2.extend_from_slice(&0u64.to_le_bytes());
        type2.extend_from_slice(&0u16.to_le_bytes());
        type2.extend_from_slice(&0u16.to_le_bytes());
        type2.extend_from_slice(&48u32.to_le_bytes());
        type2.extend_from_slice(&target);
        let chal = parse_type2(&base64_encode(&type2)).unwrap();
        assert_eq!(hex(&chal.server_challenge), "0123456789abcdef");
        assert_eq!(chal.target_name, "DOMAIN");
        let type3 = build_type3(
            "User",
            "Domain",
            "FETCHLINE",
            "Password",
            &chal,
            &hex8("aaaaaaaaaaaaaaaa"),
            132223104000000000,
        );
        assert_eq!(&type3[0..8], b"NTLMSSP\0");
        assert_eq!(u32::from_le_bytes(type3[8..12].try_into().unwrap()), 3);
        // The NT response inside the Type 3 is the NTLMv2 response
        // for this challenge (empty target info here). Layout: LM
        // field at 12, NT field at 20 (len 20..22, offset 24..28).
        let nt_len = u16::from_le_bytes(type3[20..22].try_into().unwrap()) as usize;
        let nt_off = u32::from_le_bytes(type3[24..28].try_into().unwrap()) as usize;
        let nt_resp = &type3[nt_off..nt_off + nt_len];
        let nt = ntlm_hash("Password");
        let v2 = ntlmv2_hash(&nt, "User", "Domain");
        assert_eq!(
            nt_resp,
            ntlmv2_response(&v2, &hex8("0123456789abcdef"), &hex8("aaaaaaaaaaaaaaaa"), 132223104000000000, &[]).as_slice()
        );
    }

    #[test]
    fn split_user_handles_domain_forms() {
        assert_eq!(split_user("DOMAIN\\alice"), ("DOMAIN".to_string(), "alice".to_string()));
        assert_eq!(split_user("alice@domain.example"), ("domain.example".to_string(), "alice".to_string()));
        assert_eq!(split_user("alice"), (String::new(), "alice".to_string()));
    }

    fn hex_bytes(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex8(s: &str) -> [u8; 8] {
        hex_bytes(s).try_into().unwrap()
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        hex_bytes(s)
    }
}
