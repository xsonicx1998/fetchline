//! Proxy configuration (S09 Proxy/Socks tab + `proxy_configs` table).
//!
//! What is supported, honestly:
//! - HTTP proxy: reqwest `Proxy::http` / `Proxy::https` / `Proxy::all`,
//!   with Basic auth (`Proxy-Authorization`). This is what the engine
//!   actually routes through.
//! - SOCKS5 proxy: reqwest `Proxy::all("socks5://…")` (the `socks`
//!   feature of reqwest), with username/password auth.
//! - FTP proxy: accepted and stored (the schema allows it), and
//!   APPLIED since the FTP transport landed (see `ftp.rs`): an FTP
//!   proxy routes `ftp://` downloads, with proxy `USER`/`PASS`
//!   auth and the bypass list. It still does not apply to HTTP(S)
//!   traffic — an FTP proxy never routes a web download, and FTP
//!   through an HTTP proxy is out of scope (refused with a clear
//!   error, not silently direct).
//! - Proxy authentication (`auth`, see below): `basic`, `ntlm`,
//!   `kerberos`, or `negotiate-auto` (the default). NTLMv2 is
//!   supported for plain HTTP proxies (see `ntlm.rs` — Type 1/2/3
//!   handshake, auto-negotiated on a 407). Kerberos / SPNEGO
//!   (`Negotiate`) is supported for plain HTTP proxies too (see
//!   `kerberos.rs` — the system GSSAPI on Linux, SSPI on Windows).
//!   HTTPS through an NTLM or Kerberos proxy is not supported
//!   (it would need a TLS tunnel after the handshake). Basic auth
//!   is what reqwest sends for HTTP/SOCKS proxies when the auth
//!   setting says so.
//!
//! Fallback order (the single source of truth is
//! `proxy_auth_order`, unit-tested in this module): when the proxy
//! answers 407, the engine tries the offered schemes in this
//! order — **Kerberos, then NTLM, then Basic** — restricted to
//! what the proxy actually offered and what the `auth` setting
//! allows. `negotiate-auto` tries all three in that order; the
//! other settings try exactly one scheme and fail with a clear
//! error when the proxy does not offer it. Basic is only tried
//! when the proxy offers it (or offers nothing at all, for old
//! proxies that 407 without a `Proxy-Authenticate` header): a
//! 407 that offers only `Negotiate` never falls through to Basic
//! — it either succeeds via Kerberos or fails with a clear error
//! naming Kerberos. Basic credentials are sent preemptively only
//! when `auth` is `basic` (or for SOCKS5, where username/password
//! is the only auth); otherwise the engine waits for the proxy's
//! 407 challenge and picks the strongest offered scheme, like a
//! browser. The saved proxy username/password are used for the
//! NTLM and Basic attempts only — Kerberos uses the system ticket
//! cache / Windows sign-in, never the saved password.
//!
//! The password is never returned to the UI: `ProxyConfigRow` only
//! carries `has_password`. The decrypted password lives only inside
//! `ProxyConfig`, which is never serialised.

use reqwest::{Client, NoProxy, Proxy};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::error::{EngineError, Result};

/// The `auth` values a proxy can be configured with.
pub const PROXY_AUTH_BASIC: &str = "basic";
pub const PROXY_AUTH_NTLM: &str = "ntlm";
pub const PROXY_AUTH_KERBEROS: &str = "kerberos";
/// The default: try Kerberos, then NTLM, then Basic (see the
/// module docs for the exact order rule).
pub const PROXY_AUTH_NEGOTIATE_AUTO: &str = "negotiate-auto";

/// Resolve a stored `auth` value: a known value is kept, anything
/// else (including an empty string from an older row) means the
/// default. The store validates on save; this is defence in depth
/// for rows written before the column existed.
pub fn resolve_proxy_auth(auth: &str) -> &str {
    match auth {
        PROXY_AUTH_BASIC | PROXY_AUTH_NTLM | PROXY_AUTH_KERBEROS | PROXY_AUTH_NEGOTIATE_AUTO => auth,
        _ => PROXY_AUTH_NEGOTIATE_AUTO,
    }
}

/// One proxy-authentication scheme, as offered by a proxy's
/// `Proxy-Authenticate` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyAuthScheme {
    /// `Negotiate` — SPNEGO/Kerberos (see `kerberos.rs`).
    Kerberos,
    /// `NTLM` (see `ntlm.rs`).
    Ntlm,
    /// `Basic`.
    Basic,
}

/// Parse `Proxy-Authenticate` header values into the schemes the
/// proxy offers (de-duplicated, in first-seen order — the order
/// here does not matter: `proxy_auth_order` imposes its own).
/// Unknown schemes are ignored, not rejected.
pub fn offered_proxy_auth_schemes<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<ProxyAuthScheme> {
    let mut out: Vec<ProxyAuthScheme> = Vec::new();
    for value in values {
        let scheme = value.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        let parsed = match scheme.as_str() {
            "negotiate" => ProxyAuthScheme::Kerberos,
            "ntlm" => ProxyAuthScheme::Ntlm,
            "basic" => ProxyAuthScheme::Basic,
            _ => continue,
        };
        if !out.contains(&parsed) {
            out.push(parsed);
        }
    }
    out
}

/// The authentication order for this proxy config against what the
/// proxy offered — the single source of truth for the fallback
/// order (see the module docs). `has_basic_creds` is whether a
/// proxy username + password are configured (Basic needs them;
/// Kerberos never does, NTLM does).
///
/// - `negotiate-auto`: Kerberos (if offered), then NTLM (if
///   offered), then Basic (if offered and credentials exist). When
///   the proxy offered nothing at all, Basic is the only move when
///   credentials exist.
/// - `kerberos` / `ntlm`: exactly that scheme, and only when the
///   proxy offers it — otherwise the order is empty and the caller
///   must fail with a clear error naming the selected scheme,
///   never silently try another one.
/// - `basic`: exactly Basic (credentials are sent preemptively by
///   the client, so a 407 here means they were refused).
pub fn proxy_auth_order(
    auth: &str,
    offered: &[ProxyAuthScheme],
    has_basic_creds: bool,
) -> Vec<ProxyAuthScheme> {
    match resolve_proxy_auth(auth) {
        PROXY_AUTH_BASIC => {
            if has_basic_creds {
                vec![ProxyAuthScheme::Basic]
            } else {
                vec![]
            }
        }
        PROXY_AUTH_KERBEROS => {
            if offered.contains(&ProxyAuthScheme::Kerberos) {
                vec![ProxyAuthScheme::Kerberos]
            } else {
                vec![]
            }
        }
        PROXY_AUTH_NTLM => {
            if offered.contains(&ProxyAuthScheme::Ntlm) {
                vec![ProxyAuthScheme::Ntlm]
            } else {
                vec![]
            }
        }
        _ => {
            let mut order = Vec::new();
            if offered.contains(&ProxyAuthScheme::Kerberos) {
                order.push(ProxyAuthScheme::Kerberos);
            }
            if offered.contains(&ProxyAuthScheme::Ntlm) {
                order.push(ProxyAuthScheme::Ntlm);
            }
            if has_basic_creds && (offered.contains(&ProxyAuthScheme::Basic) || offered.is_empty()) {
                order.push(ProxyAuthScheme::Basic);
            }
            order
        }
    }
}

/// The `Proxy-Authorization` header value for Basic proxy auth
/// (`Basic <base64(user:password)>`). Used for the per-request
/// Basic retry after a 407 — reqwest has no per-request proxy-auth
/// API, and the client-level `Proxy::basic_auth` is only applied
/// when the auth setting is `basic` (see `to_reqwest_proxy`).
pub fn basic_proxy_auth_header(user: &str, password: &str) -> String {
    format!("Basic {}", crate::secret::base64_encode(format!("{user}:{password}").as_bytes()))
}

/// The error for a 407 whose offered schemes do not include the
/// one the proxy settings selected — a refusal, never a silent
/// try of another scheme.
pub fn proxy_auth_not_offered_error(auth: &str, offered: &[ProxyAuthScheme]) -> EngineError {
    let offered_list = if offered.is_empty() {
        "no authentication scheme".to_string()
    } else {
        offered
            .iter()
            .map(|s| match s {
                ProxyAuthScheme::Kerberos => "Negotiate (Kerberos)",
                ProxyAuthScheme::Ntlm => "NTLM",
                ProxyAuthScheme::Basic => "Basic",
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    match resolve_proxy_auth(auth) {
        PROXY_AUTH_KERBEROS => EngineError::InvalidState(format!(
            "This proxy offers only {offered_list}, but Kerberos (Negotiate) is selected in the proxy settings — change the proxy authentication or use a proxy that offers Negotiate"
        )),
        PROXY_AUTH_NTLM => EngineError::InvalidState(format!(
            "This proxy offers only {offered_list}, but NTLM is selected in the proxy settings — change the proxy authentication or use a proxy that offers NTLM"
        )),
        _ => EngineError::InvalidState(
            "The proxy needs Basic authentication — set the proxy username and password".to_string(),
        ),
    }
}

/// One proxy row, as the UI sees it. No password — only whether one
/// is stored, so the dialog can show "password saved" without ever
/// receiving the secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfigRow {
    pub id: String,
    pub name: String,
    /// "http" | "socks" | "ftp" (schema CHECK values).
    pub proxy_type: String,
    pub host: String,
    pub port: i64,
    pub username: Option<String>,
    /// Protocol scope: [] = all traffic, ["http"] = HTTP only,
    /// ["https"] = HTTPS only.
    pub use_for: Vec<String>,
    /// Hosts that bypass the proxy (comma-separated in the UI).
    pub bypass: Vec<String>,
    pub enabled: bool,
    pub has_password: bool,
    /// Proxy authentication: `basic` | `ntlm` | `kerberos` |
    /// `negotiate-auto` (the default — see the module docs).
    pub auth: String,
}

/// What the UI sends when saving a proxy. `password: None` means
/// "keep the stored password"; `Some("")` clears it; `Some(p)` sets it.
/// `auth: None` means "keep the stored authentication setting"
/// (`negotiate-auto` for a new proxy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfigInput {
    pub proxy_type: String,
    pub host: String,
    pub port: i64,
    pub username: Option<String>,
    pub password: Option<String>,
    pub use_for: Vec<String>,
    pub bypass: Vec<String>,
    pub enabled: bool,
    #[serde(default)]
    pub auth: Option<String>,
}

/// A proxy with its decrypted password, for the engine only.
/// Deliberately NOT Serialize — this must never cross the Tauri bridge.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub proxy_type: String,
    pub host: String,
    pub port: i64,
    pub username: Option<String>,
    pub password: Option<String>,
    pub use_for: Vec<String>,
    pub bypass: Vec<String>,
    /// `basic` | `ntlm` | `kerberos` | `negotiate-auto` — always
    /// resolved (see `resolve_proxy_auth`), never empty.
    pub auth: String,
}

impl ProxyConfig {
    /// Whether a proxy username + password are configured (what
    /// the NTLM and Basic attempts need; Kerberos never does).
    pub fn has_basic_creds(&self) -> bool {
        self.username.as_deref().map(|u| !u.trim().is_empty()).unwrap_or(false)
            && self.password.as_deref().map(|p| !p.is_empty()).unwrap_or(false)
    }

    /// "host:port", with IPv6 hosts bracketed.
    fn authority(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Build the reqwest `Proxy` for this config, or `None` when the
    /// type cannot be applied to HTTP(S) traffic (FTP). The caller
    /// decides what "not applied" means; this module never pretends
    /// an FTP proxy routes traffic.
    pub fn to_reqwest_proxy(&self) -> Result<Option<Proxy>> {
        let url = match self.proxy_type.as_str() {
            "http" => format!("http://{}", self.authority()),
            "socks" => format!("socks5://{}", self.authority()),
            "ftp" => return Ok(None),
            other => {
                return Err(EngineError::InvalidState(format!(
                    "Unknown proxy type: {other}"
                )))
            }
        };
        let mut proxy = match self.use_for.as_slice() {
            [] => Proxy::all(&url)?,
            [one] if one == "http" => Proxy::http(&url)?,
            [one] if one == "https" => Proxy::https(&url)?,
            _ => Proxy::all(&url)?,
        };
        if let Some(user) = &self.username {
            // Basic credentials go on the client preemptively only
            // when the auth setting is `basic` — or always for
            // SOCKS5, where username/password is the only auth
            // there is. For `negotiate-auto` / `ntlm` / `kerberos`
            // on an HTTP proxy the engine waits for the proxy's
            // 407 challenge and picks the strongest offered scheme
            // (see the module docs) — sending Basic first would let
            // a proxy that offers Negotiate silently settle for
            // Basic instead.
            if !user.trim().is_empty()
                && (self.proxy_type == "socks" || resolve_proxy_auth(&self.auth) == PROXY_AUTH_BASIC)
            {
                proxy = proxy.basic_auth(user, self.password.as_deref().unwrap_or(""));
            }
        }
        if !self.bypass.is_empty() {
            proxy = proxy.no_proxy(NoProxy::from_string(&self.bypass.join(",")));
        }
        Ok(Some(proxy))
    }
}

/// Build the HTTP client the engine uses: connect timeout from
/// settings, and the active proxy when one is configured. Every
/// request the engine makes (probe, segment, page fetch) goes
/// through this client, so a configured proxy really is used.
///
/// Round 55 — total timeout: a request that must finish on its own
/// (probe, page fetch, update check) gets a total timeout, so a
/// stalled server can no longer block the Add dialog for minutes
/// (defect 3b: the 1 GB add hung silently). Segment body streams
/// do NOT use this client — a slow download legitimately takes
/// longer than 30 s, so segments use `build_segment_client`
/// (connect timeout only) plus a per-chunk stall watchdog
/// (`SEGMENT_STALL_TIMEOUT` in manager.rs) instead.
/// Round 56 (§7.5): 60 s → 30 s — the r55 live test showed a
/// 60 s probe failure is already past the point where the Add
/// dialog feels hung, and the dialog's own 60 s client-side
/// submit timeout still bounds the whole submit.
pub const CLIENT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Round 71 (B1) — a dropped SYN costs the full Windows TCP
/// connect timeout (~21 s: 3 + 6 + 12 s) before the request fails.
/// OVH drops SYNs when too many parallel connections arrive from
/// one IP, so a 30 s connect timeout turns every rate-limited
/// segment into a 21 s stall. Cap the connect timeout at 8 s in
/// BOTH builders (probe and segment): a server that cannot accept
/// a TCP handshake in 8 s is effectively unreachable, and failing
/// fast is what lets the per-segment deferral (manager.rs) drop
/// the excess concurrency instead of hanging the download. The
/// user's setting still governs shorter values.
pub const MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

pub fn build_client(proxy: Option<&ProxyConfig>, timeout_secs: u64) -> Result<Client> {
    build_client_with_total_timeout(proxy, timeout_secs, CLIENT_TOTAL_TIMEOUT)
}

/// `build_client` with an explicit total timeout: the production
/// path passes `CLIENT_TOTAL_TIMEOUT`; tests pass a short timeout
/// so a stalling server fails in seconds instead of a minute.
pub fn build_client_with_total_timeout(
    proxy: Option<&ProxyConfig>,
    timeout_secs: u64,
    total_timeout: Duration,
) -> Result<Client> {
    let builder = Client::builder()
        .user_agent("Fetchline/0.1")
        .connect_timeout(Duration::from_secs(timeout_secs.max(1)).min(MAX_CONNECT_TIMEOUT))
        .timeout(total_timeout);
    Ok(build_with_proxy(builder, proxy)?.build()?)
}

/// The client segment downloads use: connect timeout only, no
/// total timeout — a segment's body legitimately streams for
/// longer than any total budget on a slow connection (the test
/// fixture serves 20 MiB at 32 KiB/s, ~10 minutes). Stalls are
/// caught per chunk (`SEGMENT_STALL_TIMEOUT` in manager.rs) and
/// by the run watchdog instead of a total budget.
pub fn build_segment_client(proxy: Option<&ProxyConfig>, timeout_secs: u64) -> Result<Client> {
    let builder = Client::builder()
        .user_agent("Fetchline/0.1")
        .connect_timeout(Duration::from_secs(timeout_secs.max(1)).min(MAX_CONNECT_TIMEOUT));
    Ok(build_with_proxy(builder, proxy)?.build()?)
}

/// Apply the active proxy to a client builder (shared by the
/// total-timeout client and the segment client).
fn build_with_proxy(
    mut builder: reqwest::ClientBuilder,
    proxy: Option<&ProxyConfig>,
) -> Result<reqwest::ClientBuilder> {
    if let Some(cfg) = proxy {
        if let Some(p) = cfg.to_reqwest_proxy()? {
            builder = builder.proxy(p);
        }
    }
    Ok(builder)
}

/// Validate the fields the UI sends before anything is stored.
/// The password is not validated here — it is validated by the
/// proxy when it is used.
pub fn validate_proxy_input(input: &ProxyConfigInput) -> Result<()> {
    if !matches!(input.proxy_type.as_str(), "http" | "socks" | "ftp") {
        return Err(EngineError::InvalidState(format!(
            "Unknown proxy type: {}",
            input.proxy_type
        )));
    }
    let host = input.host.trim();
    if host.is_empty() {
        return Err(EngineError::InvalidState("Give the proxy a host".to_string()));
    }
    if host.contains("://") || host.contains(char::is_whitespace) {
        return Err(EngineError::InvalidState(
            "The proxy host is just a name or address, without http:// and without spaces".to_string(),
        ));
    }
    if !(1..=65535).contains(&input.port) {
        return Err(EngineError::InvalidState(
            "The proxy port must be between 1 and 65535".to_string(),
        ));
    }
    if let Some(auth) = &input.auth {
        if !matches!(
            auth.as_str(),
            PROXY_AUTH_BASIC | PROXY_AUTH_NTLM | PROXY_AUTH_KERBEROS | PROXY_AUTH_NEGOTIATE_AUTO
        ) {
            return Err(EngineError::InvalidState(format!(
                "Unknown proxy authentication: {auth}"
            )));
        }
        // Kerberos and NTLM are HTTP-proxy schemes only: a SOCKS5
        // proxy authenticates with username/password, and an FTP
        // proxy with USER/PASS — neither offers Negotiate/NTLM.
        if matches!(auth.as_str(), PROXY_AUTH_KERBEROS | PROXY_AUTH_NTLM) && input.proxy_type != "http" {
            return Err(EngineError::InvalidState(
                "Kerberos and NTLM proxy authentication only apply to HTTP proxies — \
                 a SOCKS5 or FTP proxy uses the username and password"
                    .to_string(),
            ));
        }
    }
    for scope in &input.use_for {
        if !matches!(scope.as_str(), "http" | "https") {
            return Err(EngineError::InvalidState(format!(
                "Unknown protocol in “use proxy for”: {scope}"
            )));
        }
    }
    for host in &input.bypass {
        if host.trim().is_empty() || host.contains(char::is_whitespace) {
            return Err(EngineError::InvalidState(
                "Bypass hosts are comma-separated names, without spaces".to_string(),
            ));
        }
    }
    Ok(())
}

/// Split a comma-separated bypass list from the UI into host names.
pub fn parse_bypass_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect()
}

/// Does `host` bypass the proxy? Exact match, or any subdomain of
/// a listed host (`sub.example.com` bypasses `example.com`) — the
/// same rule reqwest's `NoProxy` applies to HTTP traffic, so FTP
/// and HTTP agree on what "bypass" means. A listed `*` bypasses
/// everything.
pub fn host_bypasses_proxy(host: &str, bypass: &[String]) -> bool {
    let host = host.trim().to_ascii_lowercase();
    bypass.iter().any(|entry| {
        let entry = entry.trim().to_ascii_lowercase();
        entry == "*"
            || host == entry
            || (entry.starts_with('.') && host.ends_with(&entry))
            || host.ends_with(&format!(".{entry}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offered_schemes_parse_proxy_authenticate_values() {
        use ProxyAuthScheme::*;
        assert_eq!(
            offered_proxy_auth_schemes(["Negotiate", "NTLM", "Basic realm=\"proxy\""].iter().copied()),
            vec![Kerberos, Ntlm, Basic]
        );
        // Case-insensitive, de-duplicated, unknown schemes ignored.
        assert_eq!(
            offered_proxy_auth_schemes(["negotiate", "NEGOTIATE", "Digest", "ntlm"].iter().copied()),
            vec![Kerberos, Ntlm]
        );
        // A token after the scheme is part of the value, not a scheme.
        assert_eq!(
            offered_proxy_auth_schemes(["Negotiate aGVsbG8="].iter().copied()),
            vec![Kerberos]
        );
        assert!(offered_proxy_auth_schemes(std::iter::empty::<&str>()).is_empty());
    }

    #[test]
    fn fallback_order_is_kerberos_then_ntlm_then_basic() {
        use ProxyAuthScheme::*;
        let all = [Kerberos, Ntlm, Basic];
        // negotiate-auto: the full order, whatever order the proxy
        // listed the schemes in.
        assert_eq!(
            proxy_auth_order(PROXY_AUTH_NEGOTIATE_AUTO, &all, true),
            vec![Kerberos, Ntlm, Basic]
        );
        assert_eq!(
            proxy_auth_order(PROXY_AUTH_NEGOTIATE_AUTO, &[Basic, Ntlm, Kerberos], true),
            vec![Kerberos, Ntlm, Basic]
        );
        // A 407 that offers only Negotiate never falls through to
        // Basic — Basic is not in the order even with credentials.
        assert_eq!(
            proxy_auth_order(PROXY_AUTH_NEGOTIATE_AUTO, &[Kerberos], true),
            vec![Kerberos]
        );
        // Kerberos fails, NTLM is offered: NTLM is next, then Basic.
        assert_eq!(
            proxy_auth_order(PROXY_AUTH_NEGOTIATE_AUTO, &[Ntlm, Basic], true),
            vec![Ntlm, Basic]
        );
        // No Basic credentials: Basic drops out of the order.
        assert_eq!(
            proxy_auth_order(PROXY_AUTH_NEGOTIATE_AUTO, &all, false),
            vec![Kerberos, Ntlm]
        );
        // A proxy that 407s without offering anything: Basic is the
        // only move when credentials exist, nothing when they do not.
        assert_eq!(
            proxy_auth_order(PROXY_AUTH_NEGOTIATE_AUTO, &[], true),
            vec![Basic]
        );
        assert_eq!(proxy_auth_order(PROXY_AUTH_NEGOTIATE_AUTO, &[], false), Vec::<ProxyAuthScheme>::new());
    }

    #[test]
    fn explicit_auth_settings_try_exactly_one_scheme() {
        use ProxyAuthScheme::*;
        let all = [Kerberos, Ntlm, Basic];
        assert_eq!(proxy_auth_order(PROXY_AUTH_KERBEROS, &all, true), vec![Kerberos]);
        assert_eq!(proxy_auth_order(PROXY_AUTH_NTLM, &all, true), vec![Ntlm]);
        assert_eq!(proxy_auth_order(PROXY_AUTH_BASIC, &all, true), vec![Basic]);
        // …and an empty order when the proxy does not offer the
        // selected scheme — the caller must fail clearly, never
        // silently try another scheme.
        assert_eq!(proxy_auth_order(PROXY_AUTH_KERBEROS, &[Ntlm, Basic], true), Vec::<ProxyAuthScheme>::new());
        assert_eq!(proxy_auth_order(PROXY_AUTH_KERBEROS, &[Basic], true), Vec::<ProxyAuthScheme>::new());
        assert_eq!(proxy_auth_order(PROXY_AUTH_NTLM, &[Kerberos], true), Vec::<ProxyAuthScheme>::new());
        assert_eq!(proxy_auth_order(PROXY_AUTH_BASIC, &all, false), Vec::<ProxyAuthScheme>::new());
        // Unknown stored values resolve to negotiate-auto.
        assert_eq!(resolve_proxy_auth(""), PROXY_AUTH_NEGOTIATE_AUTO);
        assert_eq!(resolve_proxy_auth("digest"), PROXY_AUTH_NEGOTIATE_AUTO);
        assert_eq!(resolve_proxy_auth("kerberos"), PROXY_AUTH_KERBEROS);
    }

    #[test]
    fn validate_proxy_input_rejects_bad_auth_and_wrong_type() {
        let mut input = ProxyConfigInput {
            proxy_type: "http".to_string(),
            host: "proxy.example".to_string(),
            port: 8080,
            username: None,
            password: None,
            use_for: vec![],
            bypass: vec![],
            enabled: true,
            auth: Some("digest".to_string()),
        };
        assert!(validate_proxy_input(&input).is_err());
        input.auth = Some("kerberos".to_string());
        assert!(validate_proxy_input(&input).is_ok());
        input.proxy_type = "socks".to_string();
        assert!(validate_proxy_input(&input).is_err());
        input.proxy_type = "ftp".to_string();
        assert!(validate_proxy_input(&input).is_err());
        input.auth = Some("negotiate-auto".to_string());
        assert!(validate_proxy_input(&input).is_ok());
    }
}
