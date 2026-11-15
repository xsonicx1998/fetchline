//! Secret storage for proxy passwords (and, later, site logins).
//!
//! The schema column is `password_enc`: on Windows the value is
//! protected with DPAPI (`CryptProtectData`, user scope) through
//! direct FFI to crypt32 — no extra crate is needed, and the FFI
//! code only compiles on Windows. On any other OS there is no DPAPI,
//! so the value is only base64-obfuscated, which is NOT encryption:
//! it stops a casual glance at the database and nothing more. The
//! build log says this plainly. The `dpapi:` / `b64:` prefix on the
//! stored value tells the reader which one it is, and a value with
//! no prefix is read back as-is (rows written before this module
//! existed).

use crate::error::{EngineError, Result};

/// Protect a plaintext secret for storage. Never log the return value.
pub fn protect_secret(plain: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        if let Some(enc) = dpapi::protect(plain.as_bytes()) {
            return format!("dpapi:{}", base64_encode(&enc));
        }
        // DPAPI failed (locked profile, service account, …): fall
        // through to the obfuscated form rather than refusing to
        // save the proxy at all — the build log records this.
    }
    format!("b64:{}", base64_encode(plain.as_bytes()))
}

/// Recover a stored secret. Never log the return value.
pub fn unprotect_secret(stored: &str) -> Result<String> {
    if let Some(rest) = stored.strip_prefix("dpapi:") {
        #[cfg(target_os = "windows")]
        {
            let bytes = base64_decode(rest)
                .ok_or_else(|| EngineError::InvalidState("Stored password is corrupt".to_string()))?;
            let plain = dpapi::unprotect(&bytes)
                .ok_or_else(|| EngineError::InvalidState("Windows could not unlock the stored password".to_string()))?;
            return String::from_utf8(plain)
                .map_err(|_| EngineError::InvalidState("Stored password is corrupt".to_string()));
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = rest;
            return Err(EngineError::InvalidState(
                "This password was protected with Windows DPAPI and cannot be read on this system".to_string(),
            ));
        }
    }
    if let Some(rest) = stored.strip_prefix("b64:") {
        let bytes = base64_decode(rest)
            .ok_or_else(|| EngineError::InvalidState("Stored password is corrupt".to_string()))?;
        return String::from_utf8(bytes)
            .map_err(|_| EngineError::InvalidState("Stored password is corrupt".to_string()));
    }
    // No prefix: a plaintext row from before this module existed.
    Ok(stored.to_string())
}

/// Minimal base64 (standard alphabet, padded). No extra dependency.
pub fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut values = Vec::with_capacity(input.len());
    for c in input.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        values.push(v);
    }
    let mut out = Vec::with_capacity(values.len() * 3 / 4);
    for chunk in values.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let n = ((chunk[0] as u32) << 18)
            | ((chunk[1] as u32) << 12)
            | ((*chunk.get(2).unwrap_or(&0) as u32) << 6)
            | (*chunk.get(3).unwrap_or(&0) as u32);
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(target_os = "windows")]
mod dpapi {
    use std::ffi::c_void;

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            p_data_in: *const DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    /// CRYPTPROTECT_UI_FORBIDDEN: never show a dialog for a background save.
    const UI_FORBIDDEN: u32 = 0x1;

    pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
        let input = DataBlob { cb_data: plain.len() as u32, pb_data: plain.as_ptr() as *mut u8 };
        let mut output = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 || output.pb_data.is_null() {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() };
        unsafe { LocalFree(output.pb_data as *mut c_void) };
        Some(bytes)
    }

    pub fn unprotect(enc: &[u8]) -> Option<Vec<u8>> {
        let input = DataBlob { cb_data: enc.len() as u32, pb_data: enc.as_ptr() as *mut u8 };
        let mut output = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 || output.pb_data.is_null() {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() };
        unsafe { LocalFree(output.pb_data as *mut c_void) };
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        for s in ["", "a", "ab", "abc", "password123!@#", "pässwörd ✓"] {
            let enc = base64_encode(s.as_bytes());
            assert_eq!(base64_decode(&enc).unwrap(), s.as_bytes(), "for {s:?}");
        }
        assert!(base64_decode("not!base64").is_none());
    }

    #[test]
    fn protect_round_trip() {
        // The stored form is platform-dependent: `dpapi:` on
        // Windows when DPAPI works, `b64:` elsewhere — and `b64:`
        // on Windows too when DPAPI fails (locked profile,
        // service account). The test pins the round-trip and the
        // two known prefixes, never which one, so it is
        // deterministic on every platform. (BUG-032: the old
        // test asserted `b64:` unconditionally and failed on
        // Windows whenever DPAPI worked.)
        let stored = protect_secret("hunter2");
        assert!(
            stored.starts_with("b64:") || stored.starts_with("dpapi:"),
            "{stored}"
        );
        assert_ne!(stored, "hunter2");
        assert_eq!(unprotect_secret(&stored).unwrap(), "hunter2");
        assert_eq!(unprotect_secret("legacy-plain").unwrap(), "legacy-plain");
    }
}
