//! Kerberos (SPNEGO / `Negotiate`) proxy authentication for HTTP proxies.
//!
//! What is supported, honestly:
//! - Plain `http://` downloads and probes through an HTTP proxy that
//!   demands `Negotiate`, using the system's Kerberos stack: GSSAPI
//!   (MIT Kerberos, `libgssapi_krb5`) on Linux, SSPI (`Kerberos`
//!   package, `secur32`) on Windows. The SPN is `HTTP/<proxy host>`.
//!   The token exchange is the standard SPNEGO multi-round handshake
//!   on one raw TCP connection to the proxy (like the NTLM path in
//!   `ntlm.rs`), and the request is then performed on that same
//!   connection. No Kerberos crypto is implemented here and none is
//!   faked: the tokens come from the system GSSAPI/SSPI, and a proxy
//!   that rejects them gets a clear error naming Kerberos.
//! - The proxy username/password are NOT used for Kerberos: the
//!   identity comes from the Kerberos credential cache (on Linux:
//!   `kinit`; on Windows: the sign-in session). The saved proxy
//!   credentials are only used for the NTLM / Basic fallbacks (see
//!   `proxy::proxy_auth_order`). A Kerberos-only proxy with no
//!   usable ticket fails with a clear error instead of silently
//!   falling back to Basic — Basic is only tried when the proxy
//!   offers it (see `proxy.rs`).
//! - `https://` through a Kerberos proxy is NOT supported: the
//!   CONNECT tunnel would need a TLS handshake on the authenticated
//!   socket, which is not built (same limit as NTLM). The error
//!   says so plainly.
//! - SOCKS5 / FTP proxies do not use this module: SOCKS5 has no
//!   `Negotiate` scheme in this engine (username/password only),
//!   and FTP proxy auth is `USER`/`PASS`.
//!
//! Dependency choice (build log): NO new crate. The GSSAPI calls
//! are direct FFI to the MIT Kerberos runtime library that is
//! already installed on this VM (`libgssapi_krb5.so.2` — the
//! `#[link(name = ":libgssapi_krb5.so.2")]` form links the exact
//! versioned file, so no `-dev` package, headers or pkg-config
//! entry is needed). The well-known `libgssapi` / `gssapi` crates
//! were not used: they need pkg-config + headers (`libkrb5-dev`),
//! which are not installed, and would add a build-time dependency
//! for a handful of function declarations. On Windows the same
//! direct-FFI pattern is used against `secur32` (mirroring
//! `secret.rs`' DPAPI FFI); it is written, not compiled here —
//! the Tauri shell does not build on this Linux VM at all.
//!
//! Testing (see `tests/kerberos_tests.rs`): a real KDC is out of
//! scope — no KDC is contacted anywhere. The stub proxy validates
//! the SPNEGO framing with its own hand-written DER code (it does
//! NOT share this module's encoder/decoder), so a framing bug here
//! cannot hide behind the same code on both sides. The end-to-end
//! success path uses a scripted token source through the
//! `kerberos_request_with_initiator` seam (production callers
//! always pass the real GSSAPI/SSPI initiator); what the stub
//! proves is the framing + the multi-round handshake mechanics +
//! the fallback order, NOT a real domain login. The failure path
//! (no Kerberos credentials on this VM) goes through the real
//! GSSAPI initiator and the real manager/probe paths.

use std::time::Duration;

use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::{EngineError, Result};
use crate::ntlm::{drain_body, read_response_head, NtlmBodyReader, RawHeader};
use crate::proxy::ProxyConfig;
use crate::secret::{base64_decode, base64_encode};

const KERBEROS_TIMEOUT: Duration = Duration::from_secs(30);
/// The handshake is two rounds in practice (initial token, then
/// one continuation); five is a hard stop against a proxy that
/// keeps asking forever.
const MAX_KERBEROS_ROUNDS: usize = 5;

/// The Kerberos mechanism OID (1.2.840.113554.1.2.2), DER content bytes.
const KERBEROS_OID: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 0x01, 0x02, 0x02];
/// The SPNEGO mechanism OID (1.3.6.1.5.5.2), DER content bytes.
const SPNEGO_OID: [u8; 6] = [0x2b, 0x06, 0x01, 0x05, 0x05, 0x02];

// ---------------------------------------------------------------------------
// SPNEGO tokens (RFC 4178) — DER encode / decode, pure functions.
// ---------------------------------------------------------------------------

/// A parsed SPNEGO token. Only the fields the handshake needs are
/// kept; anything else in the token is skipped, not rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpnegoToken {
    /// `[APPLICATION 0]` — the initiator's first token.
    Init {
        mech_types: Vec<Vec<u8>>,
        mech_token: Option<Vec<u8>>,
    },
    /// `[APPLICATION 1]` — the acceptor's reply (and the
    /// initiator's later tokens).
    Resp {
        /// 0 = accept-completed, 1 = accept-incomplete,
        /// 2 = reject, 3 = request-mic.
        neg_state: Option<u8>,
        response_token: Option<Vec<u8>>,
    },
}

/// DER length encoding: short form under 128, long form above.
fn der_len(len: usize) -> Vec<u8> {
    if len < 128 {
        vec![len as u8]
    } else if len < 256 {
        vec![0x81, len as u8]
    } else if len < 65536 {
        vec![0x82, (len >> 8) as u8, len as u8]
    } else {
        vec![0x83, (len >> 16) as u8, (len >> 8) as u8, len as u8]
    }
}

fn der_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend_from_slice(&der_len(content.len()));
    out.extend_from_slice(content);
    out
}

/// Read one TLV at `offset`: `(tag, content, next_offset)`.
fn der_read_tlv(bytes: &[u8], offset: usize) -> Result<(u8, &[u8], usize)> {
    let bad = || {
        EngineError::InvalidState("The proxy sent a malformed Kerberos (SPNEGO) token".to_string())
    };
    if offset + 2 > bytes.len() {
        return Err(bad());
    }
    let tag = bytes[offset];
    let first = bytes[offset + 1];
    let (len, mut pos) = if first & 0x80 == 0 {
        (first as usize, offset + 2)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || offset + 2 + n > bytes.len() {
            return Err(bad());
        }
        let mut len = 0usize;
        for b in &bytes[offset + 2..offset + 2 + n] {
            len = (len << 8) | *b as usize;
        }
        (len, offset + 2 + n)
    };
    if pos + len > bytes.len() {
        return Err(bad());
    }
    let content = &bytes[pos..pos + len];
    pos += len;
    Ok((tag, content, pos))
}

/// Walk the TLVs inside `bytes`, calling `f` for each. Stops on the
/// first error.
fn der_walk(bytes: &[u8], mut f: impl FnMut(u8, &[u8]) -> Result<()>) -> Result<()> {
    let mut pos = 0;
    while pos < bytes.len() {
        let (tag, content, next) = der_read_tlv(bytes, pos)?;
        f(tag, content)?;
        pos = next;
    }
    Ok(())
}

/// Build a `NegTokenInit` wrapping `mech_token` (the GSSAPI
/// mechanism token): `60 <len> 06 06 <SPNEGO OID> A0 <len> 30 <len>
/// A0 <len> 30 <len> 06 09 <Kerberos OID> A2 <len> 04 <len> <token>`.
/// Production GSSAPI output is already a complete NegTokenInit, so
/// this encoder is used by the test double (and pinned by the unit
/// tests against a hand-computed vector) — not by the live path.
pub fn build_neg_token_init(mech_token: &[u8]) -> Vec<u8> {
    let mut mech_types_inner = Vec::new();
    mech_types_inner.extend_from_slice(&der_tlv(0x06, &KERBEROS_OID));
    let mech_types_seq = der_tlv(0x30, &mech_types_inner);
    let mech_types = der_tlv(0xa0, &mech_types_seq);
    let mech_token_field = der_tlv(0xa2, &der_tlv(0x04, mech_token));
    let mut seq_inner = Vec::new();
    seq_inner.extend_from_slice(&mech_types);
    seq_inner.extend_from_slice(&mech_token_field);
    let seq = der_tlv(0x30, &seq_inner);
    let init = der_tlv(0xa0, &seq);
    let mut out = vec![0x06];
    out.extend_from_slice(&der_len(SPNEGO_OID.len()));
    out.extend_from_slice(&SPNEGO_OID);
    out.extend_from_slice(&init);
    let mut token = vec![0x60];
    token.extend_from_slice(&der_len(out.len()));
    token.extend_from_slice(&out);
    token
}

/// Build a `NegTokenResp`: `A1 <len> 30 <len> A0 <len> 0A 01 <state>
/// [A2 <len> 04 <len> <token>]`. Used by the test double (and the
/// stub's own encoder is independent — see the test file).
pub fn build_neg_token_resp(neg_state: u8, response_token: Option<&[u8]>) -> Vec<u8> {
    let state_field = der_tlv(0xa0, &der_tlv(0x0a, &[neg_state]));
    let mut seq_inner = state_field;
    if let Some(token) = response_token {
        seq_inner.extend_from_slice(&der_tlv(0xa2, &der_tlv(0x04, token)));
    }
    der_tlv(0xa1, &der_tlv(0x30, &seq_inner))
}

/// Parse a SPNEGO token (a `NegTokenInit` or a `NegTokenResp`).
/// Anything that is not one of those two shapes is an error — a
/// proxy that sends garbage here is not a Kerberos proxy.
pub fn parse_spnego_token(bytes: &[u8]) -> Result<SpnegoToken> {
    let bad = || {
        EngineError::InvalidState("The proxy sent a malformed Kerberos (SPNEGO) token".to_string())
    };
    if bytes.is_empty() {
        return Err(bad());
    }
    match bytes[0] {
        0x60 => {
            let (_, content, _) = der_read_tlv(bytes, 0)?;
            // Content: OID (SPNEGO), then [0] NegTokenInit.
            let (tag, oid, pos) = der_read_tlv(content, 0)?;
            if tag != 0x06 || oid != SPNEGO_OID {
                return Err(bad());
            }
            let (tag, init_seq, _) = der_read_tlv(content, pos)?;
            if tag != 0xa0 {
                return Err(bad());
            }
            // init_seq is the SEQUENCE TLV; its content is the field list.
            let (tag, fields, _) = der_read_tlv(init_seq, 0)?;
            if tag != 0x30 {
                return Err(bad());
            }
            let mut mech_types = Vec::new();
            let mut mech_token = None;
            der_walk(fields, |tag, content| {
                match tag {
                    0xa0 => {
                        // mechTypes: SEQUENCE OF OID.
                        let (_, list, _) = der_read_tlv(content, 0)?;
                        der_walk(list, |tag, oid| {
                            if tag == 0x06 {
                                mech_types.push(oid.to_vec());
                            }
                            Ok(())
                        })?;
                    }
                    0xa2 => {
                        let (_, token, _) = der_read_tlv(content, 0)?;
                        mech_token = Some(token.to_vec());
                    }
                    _ => {}
                }
                Ok(())
            })?;
            Ok(SpnegoToken::Init {
                mech_types,
                mech_token,
            })
        }
        0xa1 => {
            let (_, content, _) = der_read_tlv(bytes, 0)?;
            let (_, fields, _) = der_read_tlv(content, 0)?;
            let mut neg_state = None;
            let mut response_token = None;
            der_walk(fields, |tag, content| {
                match tag {
                    0xa0 => {
                        let (_, state, _) = der_read_tlv(content, 0)?;
                        if state.len() == 1 {
                            neg_state = Some(state[0]);
                        }
                    }
                    0xa2 => {
                        let (_, token, _) = der_read_tlv(content, 0)?;
                        response_token = Some(token.to_vec());
                    }
                    _ => {}
                }
                Ok(())
            })?;
            Ok(SpnegoToken::Resp {
                neg_state,
                response_token,
            })
        }
        _ => Err(bad()),
    }
}

// ---------------------------------------------------------------------------
// The token source: one step of the SPNEGO exchange.
// ---------------------------------------------------------------------------

/// One step of the SPNEGO exchange: the output token to send, and
/// whether the security context is established (`complete`).
#[derive(Debug, Clone)]
pub struct SpnegoStep {
    pub token: Vec<u8>,
    pub complete: bool,
}

/// Produces SPNEGO tokens, one step at a time. The production
/// implementations call the system Kerberos stack (GSSAPI on
/// Linux, SSPI on Windows); the test double in
/// `tests/kerberos_tests.rs` returns scripted tokens. The trait is
/// the only seam: production callers always construct the real
/// initiator via `new_kerberos_initiator`.
pub trait SpnegoInitiator: Send {
    /// Run one step: `input_token` is the acceptor's response token
    /// from the previous round (`None` for the first step).
    fn step(&mut self, input_token: Option<&[u8]>) -> Result<SpnegoStep>;
}

/// Build the production initiator for `proxy_host` (the SPN is
/// `HTTP/<proxy_host>`). Fails only when the target name cannot be
/// imported — a missing credential cache is reported later, by the
/// first `step`, with the GSSAPI/SSPI message.
pub fn new_kerberos_initiator(proxy_host: &str) -> Result<Box<dyn SpnegoInitiator>> {
    #[cfg(not(target_os = "windows"))]
    {
        Ok(Box::new(GssInitiator::new(proxy_host)?))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(SspiInitiator::new(proxy_host)?))
    }
}

// ---------------------------------------------------------------------------
// GSSAPI (Linux / MIT Kerberos) — direct FFI, no new crate.
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
mod gss {
    use std::ffi::c_void;

    /// `gss_buffer_desc`: `{ length, value }`.
    #[repr(C)]
    pub struct GssBuffer {
        pub length: usize,
        pub value: *mut c_void,
    }

    /// `gss_OID_desc`: `{ length, elements }`.
    #[repr(C)]
    pub struct GssOid {
        pub length: u32,
        pub elements: *mut c_void,
    }

    // The runtime library is already installed on this VM
    // (`libgssapi_krb5.so.2`); the `:file` form links the exact
    // versioned file, so no `-dev` package or pkg-config entry is
    // needed. Verified by a scratch link probe before this module
    // was written.
    #[link(name = ":libgssapi_krb5.so.2", kind = "dylib")]
    extern "C" {
        pub fn gss_import_name(
            minor_status: *mut u32,
            input_name_buffer: *const GssBuffer,
            input_name_type: *const GssOid,
            output_name: *mut *mut c_void,
        ) -> u32;
        pub fn gss_init_sec_context(
            minor_status: *mut u32,
            claimant_cred_handle: *mut c_void,
            context_handle: *mut *mut c_void,
            target_name: *mut c_void,
            mech_type: *const GssOid,
            req_flags: u32,
            time_req: u32,
            input_chan_bindings: *mut c_void,
            input_token: *const GssBuffer,
            actual_mech_type: *mut *mut GssOid,
            output_token: *mut GssBuffer,
            ret_flags: *mut u32,
            time_rec: *mut u32,
        ) -> u32;
        pub fn gss_release_buffer(minor_status: *mut u32, buffer: *mut GssBuffer) -> u32;
        pub fn gss_release_name(minor_status: *mut u32, name: *mut *mut c_void) -> u32;
        pub fn gss_delete_sec_context(
            minor_status: *mut u32,
            context_handle: *mut *mut c_void,
            output_token: *mut GssBuffer,
        ) -> u32;
        pub fn gss_display_status(
            minor_status: *mut u32,
            status_value: u32,
            status_type: i32,
            mech_type: *const GssOid,
            message_context: *mut u32,
            status_string: *mut GssBuffer,
        ) -> u32;
    }

    /// GSS_S_COMPLETE.
    pub const GSS_S_COMPLETE: u32 = 0;
    /// GSS_S_CONTINUE_NEEDED.
    pub const GSS_S_CONTINUE_NEEDED: u32 = 1;
    /// `GSS_C_GSS_CODE` for `gss_display_status`.
    pub const GSS_C_GSS_CODE: i32 = 1;
    /// `GSS_C_MECH_CODE` for `gss_display_status`.
    pub const GSS_C_MECH_CODE: i32 = 2;
    /// `GSS_C_MUTUAL_FLAG | GSS_C_REPLAY_FLAG | GSS_C_SEQUENCE_FLAG`.
    pub const REQ_FLAGS: u32 = 0x2 | 0x4 | 0x8;
}

/// The GSSAPI initiator: one `gss_init_sec_context` context for
/// `HTTP/<proxy_host>` with the SPNEGO mechanism.
#[cfg(not(target_os = "windows"))]
pub struct GssInitiator {
    target_name: *mut std::ffi::c_void,
    context: *mut std::ffi::c_void,
}

// The handles are only ever touched from the `step` call (which the
// async caller runs on a blocking thread), never shared between
// threads — the raw pointers are Send by construction, not by
// accident.
#[cfg(not(target_os = "windows"))]
unsafe impl Send for GssInitiator {}

#[cfg(not(target_os = "windows"))]
impl GssInitiator {
    pub fn new(proxy_host: &str) -> Result<Self> {
        use gss::*;
        let service = format!("HTTP/{proxy_host}");
        let input = GssBuffer {
            length: service.len(),
            value: service.as_ptr() as *mut std::ffi::c_void,
        };
        // GSS_C_NT_HOSTBASED_SERVICE (1.2.6.1.5.6.2).
        static HOSTBASED_OID: [u8; 6] = [0x2a, 0x06, 0x01, 0x05, 0x06, 0x02];
        let ntype = GssOid {
            length: HOSTBASED_OID.len() as u32,
            elements: HOSTBASED_OID.as_ptr() as *mut std::ffi::c_void,
        };
        let mut minor: u32 = 0;
        let mut target: *mut std::ffi::c_void = std::ptr::null_mut();
        let major = unsafe { gss_import_name(&mut minor, &input, &ntype, &mut target) };
        if major != GSS_S_COMPLETE || target.is_null() {
            return Err(EngineError::InvalidState(format!(
                "Kerberos (Negotiate) proxy authentication failed: could not build the service name for the proxy ({})",
                gss_status_message(major, minor)
            )));
        }
        Ok(Self {
            target_name: target,
            context: std::ptr::null_mut(),
        })
    }
}

/// Render a GSSAPI major/minor status as text (`gss_display_status`,
/// GSS code then mechanism code). Never empty: falls back to the
/// numeric codes.
#[cfg(not(target_os = "windows"))]
fn gss_status_message(major: u32, minor: u32) -> String {
    use gss::*;
    let mut parts: Vec<String> = Vec::new();
    for (status_type, value) in [(GSS_C_GSS_CODE, major), (GSS_C_MECH_CODE, minor)] {
        let mut message_context: u32 = 0;
        loop {
            let mut m: u32 = 0;
            let mut buf = GssBuffer {
                length: 0,
                value: std::ptr::null_mut(),
            };
            let rc = unsafe {
                gss_display_status(
                    &mut m,
                    value,
                    status_type,
                    std::ptr::null(),
                    &mut message_context,
                    &mut buf,
                )
            };
            if rc != GSS_S_COMPLETE || buf.value.is_null() {
                break;
            }
            let text = unsafe { std::slice::from_raw_parts(buf.value as *const u8, buf.length) };
            let text = String::from_utf8_lossy(text).trim().to_string();
            let mut m2: u32 = 0;
            unsafe { gss_release_buffer(&mut m2, &mut buf) };
            if !text.is_empty() && text != "Unknown error" {
                parts.push(text);
            }
            if message_context == 0 {
                break;
            }
        }
    }
    if parts.is_empty() {
        format!("GSSAPI error (major 0x{major:08x}, minor 0x{minor:08x})")
    } else {
        parts.join(" — ")
    }
}

#[cfg(not(target_os = "windows"))]
impl SpnegoInitiator for GssInitiator {
    fn step(&mut self, input_token: Option<&[u8]>) -> Result<SpnegoStep> {
        use gss::*;
        // SPNEGO mechanism (1.3.6.1.5.5.2).
        static SPNEGO: [u8; 6] = [0x2b, 0x06, 0x01, 0x05, 0x05, 0x02];
        let mech = GssOid {
            length: SPNEGO.len() as u32,
            elements: SPNEGO.as_ptr() as *mut std::ffi::c_void,
        };
        let (input_buf, _owned): (GssBuffer, Option<Vec<u8>>) = match input_token {
            Some(token) => (
                GssBuffer {
                    length: token.len(),
                    value: token.as_ptr() as *mut std::ffi::c_void,
                },
                None,
            ),
            None => (
                GssBuffer {
                    length: 0,
                    value: std::ptr::null_mut(),
                },
                None,
            ),
        };
        let mut minor: u32 = 0;
        let mut output = GssBuffer {
            length: 0,
            value: std::ptr::null_mut(),
        };
        let mut ret_flags: u32 = 0;
        let major = unsafe {
            gss_init_sec_context(
                &mut minor,
                std::ptr::null_mut(), // GSS_C_NO_CREDENTIAL: the default cache
                &mut self.context,
                self.target_name,
                &mech,
                REQ_FLAGS,
                0,
                std::ptr::null_mut(),
                &input_buf,
                std::ptr::null_mut(),
                &mut output,
                &mut ret_flags,
                std::ptr::null_mut(),
            )
        };
        let token = if output.value.is_null() || output.length == 0 {
            Vec::new()
        } else {
            let bytes = unsafe {
                std::slice::from_raw_parts(output.value as *const u8, output.length).to_vec()
            };
            let mut m: u32 = 0;
            unsafe { gss_release_buffer(&mut m, &mut output) };
            bytes
        };
        match major {
            GSS_S_COMPLETE => Ok(SpnegoStep { token, complete: true }),
            GSS_S_CONTINUE_NEEDED => Ok(SpnegoStep { token, complete: false }),
            _ => Err(EngineError::InvalidState(format!(
                "Kerberos (Negotiate) proxy authentication failed: {} — check that you are signed in to Kerberos (on Linux: kinit) and that the proxy trusts this account",
                gss_status_message(major, minor)
            ))),
        }
    }
}

#[cfg(not(target_os = "windows"))]
impl Drop for GssInitiator {
    fn drop(&mut self) {
        use gss::*;
        if !self.context.is_null() {
            let mut minor: u32 = 0;
            let mut out = GssBuffer {
                length: 0,
                value: std::ptr::null_mut(),
            };
            unsafe { gss_delete_sec_context(&mut minor, &mut self.context, &mut out) };
            if !out.value.is_null() {
                let mut m: u32 = 0;
                unsafe { gss_release_buffer(&mut m, &mut out) };
            }
        }
        if !self.target_name.is_null() {
            let mut minor: u32 = 0;
            unsafe { gss_release_name(&mut minor, &mut self.target_name) };
        }
    }
}

// ---------------------------------------------------------------------------
// SSPI (Windows) — direct FFI to secur32, mirroring secret.rs' DPAPI FFI.
// Never compiled on this Linux VM (the Tauri shell does not build
// here), but the engine — this module included — compiled on the
// Windows CI runner in run #4. Signatures follow the documented
// Win32 SSPI API (AcquireCredentialsHandleA /
// InitializeSecurityContextA / FreeCredentialsHandle /
// DeleteSecurityContext). `FreeContextBuffer` is deliberately NOT
// declared: SSPI never allocates a buffer for us (the output buffer
// is our own `Vec`), so there is nothing for it to free.
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod sspi {
    use std::ffi::c_void;

    /// `SecHandle`: `{ dwLower, dwUpper }` (ULONG_PTR pair).
    #[repr(C)]
    pub struct SecHandle {
        pub dw_lower: usize,
        pub dw_upper: usize,
    }

    /// `SecBuffer`: `{ cbBuffer, BufferType, pvBuffer }`.
    #[repr(C)]
    pub struct SecBuffer {
        pub cb_buffer: u32,
        pub buffer_type: u32,
        pub pv_buffer: *mut c_void,
    }

    /// `SecBufferDesc`: `{ ulVersion, cBuffers, pBuffers }`.
    #[repr(C)]
    pub struct SecBufferDesc {
        pub ul_version: u32,
        pub c_buffers: u32,
        pub p_buffers: *mut SecBuffer,
    }

    /// `TimeStamp`: `{ dwLowDateTime, dwHighDateTime }`.
    #[repr(C)]
    pub struct TimeStamp {
        pub dw_low: u32,
        pub dw_high: u32,
    }

    #[link(name = "secur32")]
    extern "system" {
        pub fn AcquireCredentialsHandleA(
            psz_principal: *const u8,
            psz_package: *const u8,
            f_credential_use: u32,
            pv_logon_id: *mut c_void,
            p_auth_data: *mut c_void,
            p_get_key_fn: *mut c_void,
            pv_get_key_argument: *mut c_void,
            ph_credential: *mut SecHandle,
            pts_expiry: *mut TimeStamp,
        ) -> i32;
        pub fn InitializeSecurityContextA(
            ph_credential: *const SecHandle,
            ph_context: *mut SecHandle,
            psz_target_name: *const u8,
            f_context_req: u32,
            reserved1: u32,
            target_data_rep: u32,
            p_input: *const SecBufferDesc,
            reserved2: u32,
            ph_new_context: *mut SecHandle,
            p_output: *mut SecBufferDesc,
            pf_context_attr: *mut u32,
            pts_expiry: *mut TimeStamp,
        ) -> i32;
        pub fn FreeCredentialsHandle(ph_credential: *mut SecHandle) -> i32;
        pub fn DeleteSecurityContext(ph_context: *mut SecHandle) -> i32;
    }

    /// `SECPKG_CRED_OUTBOUND`.
    pub const SECPKG_CRED_OUTBOUND: u32 = 2;
    /// `SECBUFFER_VERSION`.
    pub const SECBUFFER_VERSION: u32 = 0;
    /// `SECBUFFER_TOKEN`.
    pub const SECBUFFER_TOKEN: u32 = 2;
    /// `SEC_E_OK`.
    pub const SEC_E_OK: i32 = 0;
    /// `SEC_I_CONTINUE_NEEDED` (0x00090312).
    pub const SEC_I_CONTINUE_NEEDED: i32 = 0x0009_0312;
    /// `SECURITY_NATIVE_DREP`.
    pub const SECURITY_NATIVE_DREP: u32 = 0x10;
    /// `ISC_REQ_MUTUAL_AUTH | ISC_REQ_REPLAY_DETECT | ISC_REQ_SEQUENCE_DETECT`.
    pub const ISC_REQ_FLAGS: u32 = 0x2 | 0x4 | 0x8;
}

/// The SSPI initiator: the `Kerberos` package for `HTTP/<proxy host>`.
///
/// BUG-037: this used to ask SSPI for the `Negotiate` package, which
/// silently falls back to NTLM inside the package when no Kerberos
/// ticket exists — so on Windows a token was produced (and sent)
/// with no Kerberos credentials at all, while on Linux the GSSAPI
/// initiator fails cleanly with no ticket. The setting is Kerberos
/// and the fallback order (Kerberos -> NTLM -> Basic) is handled at
/// the scheme level, so ask for the `Kerberos` package directly:
/// with no ticket `InitializeSecurityContext` fails and no token is
/// ever sent; with a ticket it produces a real Kerberos AP-REQ.
#[cfg(target_os = "windows")]
pub struct SspiInitiator {
    cred: sspi::SecHandle,
    context: sspi::SecHandle,
    have_context: bool,
    target: std::ffi::CString,
}

#[cfg(target_os = "windows")]
unsafe impl Send for SspiInitiator {}

#[cfg(target_os = "windows")]
impl SspiInitiator {
    pub fn new(proxy_host: &str) -> Result<Self> {
        use sspi::*;
        let package = std::ffi::CString::new("Kerberos").unwrap();
        let mut cred = SecHandle {
            dw_lower: 0,
            dw_upper: 0,
        };
        let mut expiry = TimeStamp {
            dw_low: 0,
            dw_high: 0,
        };
        let rc = unsafe {
            AcquireCredentialsHandleA(
                std::ptr::null(),
                package.as_ptr() as *const u8,
                SECPKG_CRED_OUTBOUND,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut cred,
                &mut expiry,
            )
        };
        if rc != SEC_E_OK {
            return Err(EngineError::InvalidState(format!(
                "Kerberos (Negotiate) proxy authentication failed: could not acquire the Windows credentials for the Kerberos package (SSPI error 0x{rc:08x}) — check that you are signed in"
            )));
        }
        let target = std::ffi::CString::new(format!("HTTP/{proxy_host}")).map_err(|_| {
            EngineError::InvalidState(
                "The proxy host cannot be used as a Kerberos service name".to_string(),
            )
        })?;
        Ok(Self {
            cred,
            context: SecHandle {
                dw_lower: 0,
                dw_upper: 0,
            },
            have_context: false,
            target,
        })
    }
}

#[cfg(target_os = "windows")]
impl SpnegoInitiator for SspiInitiator {
    fn step(&mut self, input_token: Option<&[u8]>) -> Result<SpnegoStep> {
        use sspi::*;
        // Input token, when the proxy sent one. The buffer is built
        // from the token in a single assignment, so no value is ever
        // assigned and never read (BUG-028 warning fix).
        let mut in_buf: Option<SecBuffer> = input_token.map(|token| SecBuffer {
            cb_buffer: token.len() as u32,
            buffer_type: SECBUFFER_TOKEN,
            pv_buffer: token.as_ptr() as *mut std::ffi::c_void,
        });
        let mut in_desc = SecBufferDesc {
            ul_version: SECBUFFER_VERSION,
            c_buffers: 0,
            p_buffers: std::ptr::null_mut(),
        };
        if let Some(buf) = in_buf.as_mut() {
            in_desc.c_buffers = 1;
            in_desc.p_buffers = buf;
        }
        // Output buffer: Kerberos tokens fit in 16 KiB in practice;
        // SEC_E_INSUFFICIENT_MEMORY would surface as an error below.
        let mut out_storage = vec![0u8; 16384];
        let mut out_buf = SecBuffer {
            cb_buffer: out_storage.len() as u32,
            buffer_type: SECBUFFER_TOKEN,
            pv_buffer: out_storage.as_mut_ptr() as *mut std::ffi::c_void,
        };
        let mut out_desc = SecBufferDesc {
            ul_version: SECBUFFER_VERSION,
            c_buffers: 1,
            p_buffers: &mut out_buf,
        };
        let mut attrs: u32 = 0;
        let mut expiry = TimeStamp {
            dw_low: 0,
            dw_high: 0,
        };
        let ph_context = if self.have_context {
            &mut self.context as *mut SecHandle
        } else {
            std::ptr::null_mut()
        };
        let input_ptr = if input_token.is_some() {
            &in_desc as *const SecBufferDesc
        } else {
            std::ptr::null()
        };
        let rc = unsafe {
            InitializeSecurityContextA(
                &self.cred,
                ph_context,
                self.target.as_ptr() as *const u8,
                ISC_REQ_FLAGS,
                0,
                SECURITY_NATIVE_DREP,
                input_ptr,
                0,
                &mut self.context,
                &mut out_desc,
                &mut attrs,
                &mut expiry,
            )
        };
        self.have_context = true;
        let token = out_storage[..out_buf.cb_buffer as usize].to_vec();
        match rc {
            SEC_E_OK => Ok(SpnegoStep { token, complete: true }),
            SEC_I_CONTINUE_NEEDED => Ok(SpnegoStep { token, complete: false }),
            _ => Err(EngineError::InvalidState(format!(
                "Kerberos proxy authentication failed: the Windows security package returned SSPI error 0x{rc:08x} — check that you are signed in and that the proxy trusts this account"
            ))),
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for SspiInitiator {
    fn drop(&mut self) {
        use sspi::*;
        if self.have_context {
            unsafe { DeleteSecurityContext(&mut self.context) };
        }
        unsafe { FreeCredentialsHandle(&mut self.cred) };
    }
}

// ---------------------------------------------------------------------------
// The raw HTTP exchange through a Kerberos proxy.
// ---------------------------------------------------------------------------

/// Find the `Negotiate` token on a 407 response and pull the
/// acceptor's response token out of it (`NegTokenResp`). `values`
/// are the `Proxy-Authenticate` header values. Returns `None`
/// when the proxy sent no token (the usual bare `Negotiate`) or
/// the token does not parse — the handshake then starts fresh.
pub fn negotiate_response_token<'a>(values: impl Iterator<Item = &'a str>) -> Option<Vec<u8>> {
    for value in values {
        let mut parts = value.split_whitespace();
        if parts
            .next()
            .map(|s| s.eq_ignore_ascii_case("negotiate"))
            .unwrap_or(false)
        {
            if let Some(b64) = parts.next() {
                if let Some(bytes) = base64_decode(b64) {
                    if let Ok(SpnegoToken::Resp { response_token, .. }) = parse_spnego_token(&bytes)
                    {
                        return response_token;
                    }
                }
            }
        }
    }
    None
}

/// Perform one HTTP request through an HTTP proxy that demands
/// Kerberos (`Negotiate`) auth, on a single connection: run the
/// SPNEGO handshake (`step` → send token → 407 + token → step …)
/// and then return the response to the real request. Returns
/// `(status, headers, body reader)`; the caller reads the body
/// (or drops it) and must not reuse the socket. `range` is
/// `Some((start, end))` for a ranged GET, `None` for a plain
/// request. Only plain `http://` targets are supported — see the
/// module docs for why https is refused.
pub async fn kerberos_request(
    proxy: &ProxyConfig,
    method: &str,
    url: &str,
    range: Option<(u64, Option<u64>)>,
) -> Result<(u16, Vec<RawHeader>, NtlmBodyReader)> {
    kerberos_request_with_initiator(
        proxy,
        method,
        url,
        range,
        new_kerberos_initiator(&proxy.host)?,
        None,
    )
    .await
}

/// The same exchange with an explicit token source. Production
/// callers use `kerberos_request` (the real GSSAPI/SSPI
/// initiator); the tests pass a scripted initiator through this
/// seam — see the module docs. `initial_input` is the response
/// token from the 407 that triggered this call, when it carried
/// one (usually `None`: the 407 is a bare `Negotiate`).
pub async fn kerberos_request_with_initiator(
    proxy: &ProxyConfig,
    method: &str,
    url: &str,
    range: Option<(u64, Option<u64>)>,
    initiator: Box<dyn SpnegoInitiator>,
    initial_input: Option<Vec<u8>>,
) -> Result<(u16, Vec<RawHeader>, NtlmBodyReader)> {
    let parsed = reqwest::Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
    if parsed.scheme() != "http" {
        return Err(EngineError::InvalidState(
            "Kerberos (Negotiate) proxy authentication is only supported for http:// URLs — \
             https:// through a Kerberos proxy needs a TLS tunnel after the \
             handshake, which is not built"
                .to_string(),
        ));
    }
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
    let host_header = if port == 80 {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
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
        let mut initiator = initiator;
        let mut input = initial_input;
        for _round in 0..MAX_KERBEROS_ROUNDS {
            // GSSAPI/SSPI can block on the KDC or DNS: run the step
            // on a blocking thread, never on a tokio worker. The
            // initiator moves in and back out each round.
            let (back, step_result) = tokio::task::spawn_blocking(move || {
                let result = initiator.step(input.as_deref());
                (initiator, result)
            })
            .await
            .map_err(|e| {
                EngineError::InvalidState(format!(
                    "Kerberos (Negotiate) proxy authentication failed: {e}"
                ))
            })?;
            initiator = back;
            let step = step_result?;
            let auth = format!("Negotiate {}", base64_encode(&step.token));
            reader.write_all(request_line(&auth).as_bytes()).await?;
            reader.flush().await?;
            let (status, headers) = read_response_head(&mut reader).await?;
            if status != 407 {
                // The handshake finished (or the proxy accepted the
                // token at once): the response to the real request
                // is already here.
                let body = NtlmBodyReader::new(reader, &headers);
                return Ok::<_, EngineError>((status, headers, body));
            }
            // Still 407: the proxy wants another round.
            drain_body(&mut reader, &headers).await?;
            if step.complete {
                return Err(EngineError::InvalidState(
                    "The proxy refused the Kerberos (Negotiate) authentication (407) — \
                     check that the proxy trusts this account and that the Kerberos ticket is valid"
                        .to_string(),
                ));
            }
            match negotiate_response_token(
                headers
                    .iter()
                    .filter(|(n, _)| n == "proxy-authenticate")
                    .map(|(_, v)| v.as_str()),
            ) {
                Some(token) => input = Some(token),
                None => {
                    return Err(EngineError::InvalidState(
                        "Proxy authentication failed (407) — the proxy asked for Kerberos \
                         (Negotiate) authentication but sent no token to continue the handshake"
                            .to_string(),
                    ));
                }
            }
        }
        Err(EngineError::InvalidState(
            "Kerberos (Negotiate) proxy authentication did not finish after 5 rounds — \
             the proxy keeps asking for another token"
                .to_string(),
        ))
    };
    tokio::time::timeout(KERBEROS_TIMEOUT, work)
        .await
        .map_err(|_| {
            EngineError::InvalidState(
                "The proxy did not answer the Kerberos handshake in time".to_string(),
            )
        })?
}

/// HEAD a URL through a Kerberos proxy and return `(status, headers)`.
/// The (empty) body is dropped.
pub async fn kerberos_head(proxy: &ProxyConfig, url: &str) -> Result<(u16, Vec<RawHeader>)> {
    let (status, headers, mut body) = kerberos_request(proxy, "HEAD", url, None).await?;
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
    fn neg_token_init_matches_hand_computed_vector() {
        // mech_token [01 02 03]:
        //   OID TLV 06 09 2A 86 48 86 F7 12 01 02 02
        //   mechTypes SEQ 30 0B <11> ; [0] A0 0D <13>
        //   mechToken 04 03 01 02 03 ; [2] A2 05 <5>
        //   SEQ 30 16 <22> ; [0] A0 18 <24>
        //   outer 60 22 <34> (OID 06 06 2B 06 01 05 05 02 + 26)
        let token = build_neg_token_init(&[0x01, 0x02, 0x03]);
        assert_eq!(
            hex(&token),
            "602206062b0601050502a0183016a00d300b06092a864886f712010202a2050403010203"
        );
        let parsed = parse_spnego_token(&token).unwrap();
        match parsed {
            SpnegoToken::Init {
                mech_types,
                mech_token,
            } => {
                assert_eq!(mech_types, vec![KERBEROS_OID.to_vec()]);
                assert_eq!(mech_token, Some(vec![0x01, 0x02, 0x03]));
            }
            other => panic!("expected Init, got {other:?}"),
        }
    }

    #[test]
    fn neg_token_resp_round_trip() {
        // accept-incomplete (1), response token AA BB:
        //   state 0A 01 01 ; [0] A0 03 <3>
        //   token 04 02 AA BB ; [2] A2 04 <4>
        //   SEQ 30 0B <11> ; [1] A1 0D <13>
        let token = build_neg_token_resp(1, Some(&[0xaa, 0xbb]));
        assert_eq!(hex(&token), "a10d300ba0030a0101a2040402aabb");
        let parsed = parse_spnego_token(&token).unwrap();
        match parsed {
            SpnegoToken::Resp {
                neg_state,
                response_token,
            } => {
                assert_eq!(neg_state, Some(1));
                assert_eq!(response_token, Some(vec![0xaa, 0xbb]));
            }
            other => panic!("expected Resp, got {other:?}"),
        }
        // accept-completed with no token parses too.
        let done = build_neg_token_resp(0, None);
        match parse_spnego_token(&done).unwrap() {
            SpnegoToken::Resp {
                neg_state,
                response_token,
            } => {
                assert_eq!(neg_state, Some(0));
                assert_eq!(response_token, None);
            }
            other => panic!("expected Resp, got {other:?}"),
        }
    }

    #[test]
    fn parse_spnego_token_rejects_garbage() {
        assert!(parse_spnego_token(&[]).is_err());
        assert!(parse_spnego_token(b"not a token").is_err());
        // A bare Kerberos AP-REQ (0x60 with a non-SPNEGO OID) is not a NegTokenInit.
        let mut bad = build_neg_token_init(&[0x01]);
        bad[4] = 0x01; // corrupt the SPNEGO OID
        assert!(parse_spnego_token(&bad).is_err());
        // Truncated token.
        let token = build_neg_token_init(&[0x01, 0x02, 0x03]);
        assert!(parse_spnego_token(&token[..token.len() - 2]).is_err());
    }

    #[test]
    fn negotiate_response_token_reads_the_407_token() {
        let resp = build_neg_token_resp(1, Some(&[0xde, 0xad]));
        let values = [format!("Negotiate {}", base64_encode(&resp))];
        let token = negotiate_response_token(values.iter().map(|s| s.as_str())).unwrap();
        assert_eq!(token, vec![0xde, 0xad]);
        // Bare Negotiate, or a non-Negotiate scheme, has no token.
        assert!(negotiate_response_token(["Negotiate"].iter().copied()).is_none());
        assert!(negotiate_response_token(["NTLM abc"].iter().copied()).is_none());
    }

    #[test]
    fn long_form_lengths_round_trip() {
        // A 300-byte mech token forces the 0x82 long form.
        let big = vec![0x7eu8; 300];
        let token = build_neg_token_init(&big);
        match parse_spnego_token(&token).unwrap() {
            SpnegoToken::Init { mech_token, .. } => assert_eq!(mech_token, Some(big)),
            other => panic!("expected Init, got {other:?}"),
        }
    }
}
