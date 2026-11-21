//! URL probing: learn a download's size, range support, validator and
//! filename before any bytes are written.
//!
//! Flow (per `replica/architecture.md`):
//! 1. `HEAD` the URL.
//! 2. If the server refuses `HEAD` (405/501/…), fall back to
//!    `GET` with `Range: bytes=0-0` and read the `Content-Range` header.
//! 3. If size is unknown or the server refuses ranges, the download runs
//!    as a single open-ended segment.

use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, COOKIE, ETAG,
    LAST_MODIFIED, RANGE,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use crate::error::{EngineError, Result};
use crate::proxy::{
    basic_proxy_auth_header, offered_proxy_auth_schemes, proxy_auth_order, ProxyAuthScheme,
    ProxyConfig,
};

/// What the probe learned about a URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeInfo {
    /// Total size in bytes, when the server tells us.
    pub size: Option<u64>,
    /// The server said `Accept-Ranges: bytes`.
    pub accept_ranges: bool,
    /// `ETag` validator, when present. Used on resume to detect a changed file.
    pub etag: Option<String>,
    /// `Last-Modified` validator, when present.
    pub last_modified: Option<String>,
    /// Filename from `Content-Disposition`, when present.
    pub filename: Option<String>,
    /// `Content-Type` of the probed response, when present
    /// (Round 87.2): the cinejoy.pk retest showed a watch page
    /// (5.6 KB of HTML) downloading as a Finished file — the
    /// probe never looked at what the server said the bytes
    /// were. `None` when the server sent no Content-Type (some
    /// HEAD responses omit it) — an unknown type is never
    /// refused.
    pub content_type: Option<String>,
    /// Final URL after redirects.
    pub final_url: String,
}

/// Round 87.2 — the refusal message for a probed web page.
/// Shown as the probe/add error, never as a download row.
pub const HTML_PAGE_REFUSAL: &str =
    "This link points to a web page (HTML), not a downloadable video or file.";

/// Round 87.2 — true when the probed response is a web PAGE,
/// not a file: `Content-Type` is `text/html` (or
/// `application/xhtml+xml`) and the URL path does not
/// explicitly end in `.html` / `.htm` / `.mhtml`. The media
/// type is compared without its parameters (`text/html;
/// charset=utf-8` counts) and case-insensitively; the
/// extension check runs on the final URL's path (after
/// redirects, query and fragment stripped). A missing
/// Content-Type never refuses — an unknown type is not proof
/// of a page.
pub fn is_html_page_response(content_type: Option<&str>, final_url: &str) -> bool {
    let Some(media_type) = content_type
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|s| !s.is_empty())
    else {
        return false;
    };
    if media_type != "text/html" && media_type != "application/xhtml+xml" {
        return false;
    }
    let path = final_url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    !(path.ends_with(".html") || path.ends_with(".htm") || path.ends_with(".mhtml"))
}

impl ProbeInfo {
    /// Round 87.2 — refuse a probed web page as a download:
    /// `Err` with the clear message when `is_html_page_response`
    /// holds, `Ok(())` otherwise. Called by `from_headers` /
    /// `probe_info_from_raw`, so every probe path refuses — the
    /// dialog's probe, the add path and resume all fail with the
    /// same message and no row is ever created for a page.
    pub fn refuse_html_page(&self) -> Result<()> {
        if is_html_page_response(self.content_type.as_deref(), &self.final_url) {
            return Err(EngineError::InvalidState(HTML_PAGE_REFUSAL.to_string()));
        }
        Ok(())
    }
}

/// Probe `url` and return what the server tells us about the file.
pub async fn probe(client: &Client, url: &str) -> Result<ProbeInfo> {
    probe_with_auth(client, url, None).await
}

/// Probe with the browser's cookies for this URL (S13 finish):
/// `cookies` is the validated `Cookie` header value (see
/// `native_host::cookies_to_header`), or `None` for an anonymous
/// probe. The header is attached to the probe's own requests only
/// — the shared client never carries one site's session to
/// another site — so a login-only link probes (and then
/// downloads) instead of failing with 401.
///
/// A 407 from the proxy is handled by the same fallback order as
/// the plain probe (Kerberos, then NTLM, then Basic — see
/// `proxy::proxy_auth_order`), with one limit: the raw Kerberos /
/// NTLM handshakes cannot carry cookies, so a cookie-gated link
/// behind a Negotiate/NTLM-only proxy fails with a clear error
/// naming the scheme (the same documented limit as NTLM). A
/// Basic proxy re-issues the probe with the cookies and the proxy
/// credentials. FTP URLs cannot carry cookies (there is no
/// `Cookie` header in FTP) and are refused here — the caller falls
/// back to the plain probe for them.
pub async fn probe_with_cookies(
    client: &Client,
    url: &str,
    cookies: Option<&str>,
    proxy: Option<&ProxyConfig>,
) -> Result<ProbeInfo> {
    let parsed = reqwest::Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(EngineError::InvalidUrl(
            "Only http:// and https:// addresses can carry cookies".to_string(),
        ));
    }

    // Round 70 — a transport-level HEAD failure (the server kills
    // HEAD outright, ...) falls back to a ranged GET, like the
    // 405/501/403 fallback below. A 407 that surfaces as a
    // transport error is handled inside the ranged path, which
    // walks the same proxy-auth fallback with ranged=true.
    let head = match apply_cookies(client.head(url), cookies).send().await {
        Ok(r) => r,
        Err(_) => return probe_with_ranged_get_cookies(client, url, cookies, proxy).await,
    };
    let status = head.status();
    if status.as_u16() == 407 {
        return probe_cookies_407_fallback(client, url, cookies, proxy, false).await;
    }
    if status.is_success() || status == StatusCode::PARTIAL_CONTENT {
        return from_headers(head.headers(), head.url().as_str());
    }
    if status == StatusCode::METHOD_NOT_ALLOWED
        || status == StatusCode::NOT_IMPLEMENTED
        || status == StatusCode::FORBIDDEN
    {
        // Some servers refuse HEAD; a 1-byte ranged GET gives us the same facts.
        return probe_with_ranged_get_cookies(client, url, cookies, proxy).await;
    }
    if status.is_client_error() || status.is_server_error() {
        return Err(EngineError::Status(
            status.as_u16(),
            format!("{status} while checking the address"),
        ));
    }
    Err(EngineError::Status(status.as_u16(), format!("{status}")))
}

/// A 407 from the proxy on a cookie probe: the same fallback order
/// as the plain probe, except the raw Kerberos / NTLM handshakes
/// cannot carry the cookies, so those schemes fail clearly here
/// instead of being attempted.
async fn probe_cookies_407_fallback(
    client: &Client,
    url: &str,
    cookies: Option<&str>,
    proxy: Option<&ProxyConfig>,
    ranged: bool,
) -> Result<ProbeInfo> {
    let Some(cfg) = proxy.filter(|c| c.proxy_type == "http") else {
        return Err(EngineError::Status(
            407,
            "407 Proxy Authentication Required while checking the address".to_string(),
        ));
    };
    // The 407 already told us the proxy wants auth; re-issue the
    // request and read its Proxy-Authenticate header — the first
    // response's headers are gone with the response, and a second
    // 407 is cheap.
    let resp = if ranged {
        apply_cookies(client.get(url), cookies)
            .header(RANGE, "bytes=0-0")
            .send()
            .await?
    } else {
        apply_cookies(client.head(url), cookies).send().await?
    };
    let offered = offered_from_response(&resp);
    let order = proxy_auth_order(&cfg.auth, &offered, cfg.has_basic_creds());
    if order.is_empty() {
        return Err(crate::proxy::proxy_auth_not_offered_error(
            &cfg.auth, &offered,
        ));
    }
    for scheme in order {
        match scheme {
            ProxyAuthScheme::Kerberos => {
                return Err(EngineError::InvalidState(
                    "The proxy needs Kerberos (Negotiate) authentication, which cannot be combined with a cookie-gated download — the Kerberos handshake cannot carry browser cookies".to_string(),
                ));
            }
            ProxyAuthScheme::Ntlm => {
                return Err(EngineError::InvalidState(
                    "The proxy needs NTLM authentication, which cannot be combined with a cookie-gated download — the NTLM handshake cannot carry browser cookies".to_string(),
                ));
            }
            ProxyAuthScheme::Basic => {
                let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) else {
                    return Err(EngineError::InvalidState(
                        "The proxy needs Basic authentication — set the proxy username and password".to_string(),
                    ));
                };
                let resp = if ranged {
                    apply_cookies(client.get(url), cookies)
                        .header(RANGE, "bytes=0-0")
                        .header("proxy-authorization", basic_proxy_auth_header(user, pass))
                        .send()
                        .await?
                } else {
                    apply_cookies(client.head(url), cookies)
                        .header("proxy-authorization", basic_proxy_auth_header(user, pass))
                        .send()
                        .await?
                };
                let status = resp.status();
                if status.as_u16() == 407 {
                    return Err(EngineError::InvalidState(
                        "Proxy authentication failed (407) — check the proxy username and password"
                            .to_string(),
                    ));
                }
                if status.is_success() || status == StatusCode::PARTIAL_CONTENT {
                    let mut info = from_headers(resp.headers(), resp.url().as_str())?;
                    if ranged {
                        apply_ranged_total(&mut info, resp.headers());
                    }
                    return Ok(info);
                }
                if !ranged
                    && (status == StatusCode::METHOD_NOT_ALLOWED
                        || status == StatusCode::NOT_IMPLEMENTED
                        || status == StatusCode::FORBIDDEN)
                {
                    // The server refuses HEAD: a 1-byte ranged GET
                    // with the same cookies and proxy credentials
                    // gives us the same facts (inlined here, not
                    // via probe_with_ranged_get_cookies — that
                    // would recurse into this fallback).
                    let resp = apply_cookies(client.get(url), cookies)
                        .header(RANGE, "bytes=0-0")
                        .header("proxy-authorization", basic_proxy_auth_header(user, pass))
                        .send()
                        .await?;
                    let status = resp.status();
                    if status.as_u16() == 407 {
                        return Err(EngineError::InvalidState(
                            "Proxy authentication failed (407) — check the proxy username and password".to_string(),
                        ));
                    }
                    if !status.is_success() && status != StatusCode::PARTIAL_CONTENT {
                        return Err(EngineError::Status(
                            status.as_u16(),
                            format!("{status} while checking the address"),
                        ));
                    }
                    let mut info = from_headers(resp.headers(), resp.url().as_str())?;
                    apply_ranged_total(&mut info, resp.headers());
                    return Ok(info);
                }
                return Err(EngineError::Status(
                    status.as_u16(),
                    format!("{status} while checking the address"),
                ));
            }
        }
    }
    Err(EngineError::Status(
        407,
        "407 Proxy Authentication Required while checking the address".to_string(),
    ))
}

/// Attach a validated `Cookie` header value to one request
/// builder, if any. The value was validated at the host boundary
/// (`cookies_to_header`), so it cannot smuggle a second cookie or
/// a second header in.
fn apply_cookies(req: reqwest::RequestBuilder, cookies: Option<&str>) -> reqwest::RequestBuilder {
    match cookies {
        Some(value) if !value.is_empty() => req.header(COOKIE, value),
        _ => req,
    }
}

async fn probe_with_ranged_get_cookies(
    client: &Client,
    url: &str,
    cookies: Option<&str>,
    proxy: Option<&ProxyConfig>,
) -> Result<ProbeInfo> {
    let resp = apply_cookies(client.get(url), cookies)
        .header(RANGE, "bytes=0-0")
        .send()
        .await?;
    let status = resp.status();
    if status.as_u16() == 407 {
        return probe_cookies_407_fallback(client, url, cookies, proxy, true).await;
    }
    if !status.is_success() && status != StatusCode::PARTIAL_CONTENT {
        return Err(EngineError::Status(
            status.as_u16(),
            format!("{status} while checking the address"),
        ));
    }
    let mut info = from_headers(resp.headers(), resp.url().as_str())?;
    apply_ranged_total(&mut info, resp.headers());
    Ok(info)
}

/// Per-download request headers (Round 78): what yt-dlp learned
/// about a platform video — the `User-Agent` and `Referer` the
/// stream host expects (and any other non-cookie header yt-dlp
/// recorded). A download that needs them must send them on the
/// probe AND on every segment request, or the host answers 403 —
/// today segments send only Range + Cookie + proxy-auth and the
/// UA is the global "Fetchline/0.1", which these hosts reject.
/// `Cookie` keys are never carried here: cookies ride the
/// `cookies` parameter, validated at the host boundary, so a
/// header map cannot smuggle a session cookie past it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestHeaders {
    pub user_agent: Option<String>,
    pub referer: Option<String>,
}

impl RequestHeaders {
    /// True when neither header is set — the common case for a
    /// plain download.
    pub fn is_empty(&self) -> bool {
        self.user_agent
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
            && self
                .referer
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_none()
    }
}

/// Attach per-download headers to one request builder, if any.
/// The values come from yt-dlp's `http_headers` for a resolved
/// video; an empty/absent value is skipped, never sent blank.
pub fn apply_headers(
    req: reqwest::RequestBuilder,
    headers: Option<&RequestHeaders>,
) -> reqwest::RequestBuilder {
    let Some(headers) = headers else {
        return req;
    };
    let mut req = req;
    if let Some(ua) = headers
        .user_agent
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        req = req.header(reqwest::header::USER_AGENT, ua);
    }
    if let Some(referer) = headers
        .referer
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        req = req.header(reqwest::header::REFERER, referer);
    }
    req
}

/// Probe with per-download headers (Round 78): the same flow as
/// `probe_with_cookies` (HEAD, then a 1-byte ranged GET when the
/// server refuses HEAD), with the download's `User-Agent` /
/// `Referer` attached to both attempts. Cookies ride the
/// `cookies` parameter as before. A 407 is handled by the same
/// cookie fallback, which cannot carry headers through the raw
/// Kerberos/NTLM handshakes — the same documented limit as
/// cookies there.
pub async fn probe_with_headers(
    client: &Client,
    url: &str,
    cookies: Option<&str>,
    headers: Option<&RequestHeaders>,
    proxy: Option<&ProxyConfig>,
) -> Result<ProbeInfo> {
    let parsed = reqwest::Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(EngineError::InvalidUrl(
            "Only http:// and https:// addresses can carry cookies".to_string(),
        ));
    }
    let head = match apply_headers(apply_cookies(client.head(url), cookies), headers)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return probe_with_ranged_get_headers(client, url, cookies, headers, proxy).await,
    };
    let status = head.status();
    if status.as_u16() == 407 {
        return probe_cookies_407_fallback(client, url, cookies, proxy, false).await;
    }
    if status.is_success() || status == StatusCode::PARTIAL_CONTENT {
        return from_headers(head.headers(), head.url().as_str());
    }
    if status == StatusCode::METHOD_NOT_ALLOWED
        || status == StatusCode::NOT_IMPLEMENTED
        || status == StatusCode::FORBIDDEN
    {
        return probe_with_ranged_get_headers(client, url, cookies, headers, proxy).await;
    }
    if status.is_client_error() || status.is_server_error() {
        return Err(EngineError::Status(
            status.as_u16(),
            format!("{status} while checking the address"),
        ));
    }
    Err(EngineError::Status(status.as_u16(), format!("{status}")))
}

async fn probe_with_ranged_get_headers(
    client: &Client,
    url: &str,
    cookies: Option<&str>,
    headers: Option<&RequestHeaders>,
    proxy: Option<&ProxyConfig>,
) -> Result<ProbeInfo> {
    let resp = apply_headers(apply_cookies(client.get(url), cookies), headers)
        .header(RANGE, "bytes=0-0")
        .send()
        .await?;
    let status = resp.status();
    if status.as_u16() == 407 {
        return probe_cookies_407_fallback(client, url, cookies, proxy, true).await;
    }
    if !status.is_success() && status != StatusCode::PARTIAL_CONTENT {
        return Err(EngineError::Status(
            status.as_u16(),
            format!("{status} while checking the address"),
        ));
    }
    let mut info = from_headers(resp.headers(), resp.url().as_str())?;
    apply_ranged_total(&mut info, resp.headers());
    Ok(info)
}

/// Read the total size out of a 206 `Content-Range` header
/// (`bytes 0-0/<total>`) into a probe result.
fn apply_ranged_total(info: &mut ProbeInfo, headers: &reqwest::header::HeaderMap) {
    if let Some(range) = headers.get(CONTENT_RANGE).and_then(|v| v.to_str().ok()) {
        if let Some(total) = range.rsplit('/').next().and_then(|t| t.parse::<u64>().ok()) {
            info.size = Some(total);
            info.accept_ranges = true;
        }
    }
}

/// The schemes a 407 response offers, from its
/// `Proxy-Authenticate` headers.
fn offered_from_response(resp: &reqwest::Response) -> Vec<ProxyAuthScheme> {
    offered_proxy_auth_schemes(
        resp.headers()
            .get_all("proxy-authenticate")
            .iter()
            .filter_map(|v| v.to_str().ok()),
    )
}

/// Probe with HTTP Basic auth for this host (S09 Site logins):
/// `auth` is the saved `(username, password)` for the URL's host,
/// or `None` for an anonymous probe. The credential is sent only
/// on this probe's own requests, never stored on the client — a
/// shared client must never carry one site's password to another
/// site. A 401 from the probe is the server's answer, surfaced as
/// a status error like any other probe failure.
pub async fn probe_with_auth(
    client: &Client,
    url: &str,
    auth: Option<(&str, &str)>,
) -> Result<ProbeInfo> {
    probe_with_proxy(client, url, auth, None).await
}

/// Probe with the active proxy in hand, so FTP URLs and proxy auth
/// work (both need the proxy config, which the plain `probe`
/// never sees):
/// - `ftp://` URLs go to the FTP transport (`ftp::ftp_probe`) —
///   direct, or through an FTP proxy when one is configured.
/// - An HTTP(S) probe that answers 407 walks the proxy-auth
///   fallback order (Kerberos, then NTLM, then Basic — see
///   `proxy::proxy_auth_order`) for plain `http://` URLs. For
///   `https://` the error says NTLM/Kerberos are not supported
///   there (a TLS tunnel after the handshake is not built). A
///   proxy that demands only `Negotiate` (Kerberos) either
///   authenticates via the system Kerberos stack or fails with a
///   clear error naming Kerberos — it is never silently downgraded
///   to Basic.
pub async fn probe_with_proxy(
    client: &Client,
    url: &str,
    auth: Option<(&str, &str)>,
    proxy: Option<&ProxyConfig>,
) -> Result<ProbeInfo> {
    let parsed = reqwest::Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
    if parsed.scheme() == "ftp" {
        return crate::ftp::ftp_probe(url, proxy).await;
    }
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(EngineError::InvalidUrl(
            "Only http://, https:// and ftp:// addresses are supported".to_string(),
        ));
    }

    // A failed CONNECT (https through a proxy that answers 407)
    // surfaces from reqwest as a transport error, not as a 407
    // response — ask the proxy what it wants, so an NTLM or
    // Kerberos-only proxy gets a clear error instead of a bare
    // network error. Only on failure, so the fast path pays nothing.
    let head = match apply_auth(client.head(url), auth).send().await {
        Ok(r) => r,
        Err(_) => {
            if parsed.scheme() == "https" {
                if let Some(cfg) = proxy {
                    if cfg.proxy_type == "http" && cfg.username.is_some() {
                        if let Some(kind) = crate::ntlm::probe_connect_auth(cfg, url).await {
                            match kind {
                                crate::ntlm::ConnectAuthKind::Ntlm => {
                                    return Err(EngineError::InvalidState(
                                        "NTLM proxy authentication is only supported for http:// URLs — \
                                         https:// through an NTLM proxy needs a TLS tunnel after the \
                                         handshake, which is not built"
                                            .to_string(),
                                    ));
                                }
                                crate::ntlm::ConnectAuthKind::NegotiateOnly => {
                                    return Err(EngineError::InvalidState(
                                        "Kerberos (Negotiate) proxy authentication is only supported for http:// URLs — \
                                         https:// through a Kerberos proxy needs a TLS tunnel after the \
                                         handshake, which is not built"
                                            .to_string(),
                                    ));
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            // Round 70 — the HEAD itself failed at the transport
            // level (connection reset, a TLS failure, a server that
            // kills HEAD outright — the downet.net file hosts do
            // this while a plain GET downloads fine): a ranged GET
            // asks the same question another way, exactly like the
            // 405/501/403 fallback below. Only if the GET fails too
            // is the address really unreachable — and then the GET's
            // error is the one reported, since GET is the request a
            // browser would have made.
            return probe_with_ranged_get(client, url, auth, proxy).await;
        }
    };
    let status = head.status();
    if status.as_u16() == 407 {
        return probe_proxy_auth_fallback(client, url, auth, proxy, false).await;
    }
    if status.is_success() || status == StatusCode::PARTIAL_CONTENT {
        return from_headers(head.headers(), head.url().as_str());
    }
    if status == StatusCode::METHOD_NOT_ALLOWED
        || status == StatusCode::NOT_IMPLEMENTED
        || status == StatusCode::FORBIDDEN
    {
        // Some servers refuse HEAD; a 1-byte ranged GET gives us the same facts.
        return probe_with_ranged_get(client, url, auth, proxy).await;
    }
    if status.is_client_error() || status.is_server_error() {
        return Err(EngineError::Status(
            status.as_u16(),
            format!("{status} while checking the address"),
        ));
    }
    Err(EngineError::Status(status.as_u16(), format!("{status}")))
}

/// A 407 from the proxy: walk the proxy-auth fallback order
/// (Kerberos, then NTLM, then Basic — see
/// `proxy::proxy_auth_order`) for plain `http://` URLs. `ranged`
/// retries with a 1-byte ranged GET instead of a HEAD, for servers
/// that refuse HEAD. A scheme that fails (no Kerberos ticket, a
/// refused NTLM password, a refused Basic password) falls through
/// to the next scheme in the order; when the order is exhausted
/// the last failure is reported. A scheme the settings did not
/// select is never tried: an empty order fails with a clear error
/// naming the selected scheme.
async fn probe_proxy_auth_fallback(
    client: &Client,
    url: &str,
    auth: Option<(&str, &str)>,
    proxy: Option<&ProxyConfig>,
    ranged: bool,
) -> Result<ProbeInfo> {
    let plain_407 = || {
        EngineError::Status(
            407,
            "407 Proxy Authentication Required while checking the address".to_string(),
        )
    };
    let Some(cfg) = proxy else {
        return Err(plain_407());
    };
    if cfg.proxy_type != "http" {
        return Err(plain_407());
    }
    // The 407 already told us the proxy wants auth (Basic was
    // either not sent — the auth setting is not `basic` — or was
    // refused); confirm by re-issuing the request and reading its
    // Proxy-Authenticate header — the first response's headers are
    // gone with the response, and a second 407 is cheap.
    let resp = if ranged {
        apply_auth(client.get(url), auth)
            .header(RANGE, "bytes=0-0")
            .send()
            .await?
    } else {
        apply_auth(client.head(url), auth).send().await?
    };
    let offered = offered_from_response(&resp);
    let order = proxy_auth_order(&cfg.auth, &offered, cfg.has_basic_creds());
    if order.is_empty() {
        return Err(crate::proxy::proxy_auth_not_offered_error(
            &cfg.auth, &offered,
        ));
    }
    let parsed = reqwest::Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
    let mut last_err: Option<EngineError> = None;
    for scheme in order {
        match scheme {
            ProxyAuthScheme::Kerberos => {
                if parsed.scheme() != "http" {
                    return Err(EngineError::InvalidState(
                        "Kerberos (Negotiate) proxy authentication is only supported for http:// URLs — \
                         https:// through a Kerberos proxy needs a TLS tunnel after the \
                         handshake, which is not built"
                            .to_string(),
                    ));
                }
                let initial_input = crate::kerberos::negotiate_response_token(
                    resp.headers()
                        .get_all("proxy-authenticate")
                        .iter()
                        .filter_map(|v| v.to_str().ok()),
                );
                // A failure to build the initiator (or to run the
                // handshake) is a Kerberos failure: record it and
                // fall through to the next scheme in the order.
                let attempt: Result<(u16, Vec<(String, String)>)> =
                    match crate::kerberos::new_kerberos_initiator(&cfg.host) {
                        Ok(initiator) => {
                            if ranged {
                                match crate::kerberos::kerberos_request_with_initiator(
                                    cfg,
                                    "GET",
                                    url,
                                    Some((0, Some(0))),
                                    initiator,
                                    initial_input,
                                )
                                .await
                                {
                                    Ok((s, h, mut body)) => {
                                        let _ = &mut body;
                                        Ok((s, h))
                                    }
                                    Err(e) => Err(e),
                                }
                            } else {
                                crate::kerberos::kerberos_head(cfg, url).await
                            }
                        }
                        Err(e) => Err(e),
                    };
                match attempt {
                    Ok((status, headers)) => {
                        if status == 407 {
                            last_err = Some(EngineError::InvalidState(
                                "The proxy refused the Kerberos (Negotiate) authentication (407) — \
                                 check that the proxy trusts this account and that the Kerberos ticket is valid"
                                    .to_string(),
                            ));
                            continue;
                        }
                        if status >= 400 {
                            return Err(EngineError::Status(
                                status,
                                format!("{status} while checking the address"),
                            ));
                        }
                        let mut info = probe_info_from_raw(&headers, url)?;
                        if ranged {
                            if let Some(range) = headers
                                .iter()
                                .find(|(n, _)| n == "content-range")
                                .map(|(_, v)| v.as_str())
                            {
                                if let Some(total) =
                                    range.rsplit('/').next().and_then(|t| t.parse::<u64>().ok())
                                {
                                    info.size = Some(total);
                                    info.accept_ranges = true;
                                }
                            }
                        }
                        return Ok(info);
                    }
                    Err(e) => {
                        last_err = Some(e);
                        continue;
                    }
                }
            }
            ProxyAuthScheme::Ntlm => {
                if parsed.scheme() != "http" {
                    return Err(EngineError::InvalidState(
                        "NTLM proxy authentication is only supported for http:// URLs — \
                         https:// through an NTLM proxy needs a TLS tunnel after the \
                         handshake, which is not built"
                            .to_string(),
                    ));
                }
                // A failed handshake is recorded and the next scheme
                // in the order is tried, like Kerberos above.
                let attempt: Result<(u16, Vec<(String, String)>)> = if ranged {
                    match crate::ntlm::ntlm_request(cfg, "GET", url, Some((0, Some(0)))).await {
                        Ok((s, h, mut body)) => {
                            let _ = &mut body;
                            Ok((s, h))
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    crate::ntlm::ntlm_head(cfg, url).await
                };
                let (status, headers) = match attempt {
                    Ok(v) => v,
                    Err(e) => {
                        last_err = Some(e);
                        continue;
                    }
                };
                if status == 407 {
                    last_err = Some(EngineError::InvalidState(
                        "Proxy authentication failed (407) — check the proxy username and password"
                            .to_string(),
                    ));
                    continue;
                }
                if status >= 400 {
                    return Err(EngineError::Status(
                        status,
                        format!("{status} while checking the address"),
                    ));
                }
                let mut info = probe_info_from_raw(&headers, url)?;
                if ranged {
                    if let Some(range) = headers
                        .iter()
                        .find(|(n, _)| n == "content-range")
                        .map(|(_, v)| v.as_str())
                    {
                        if let Some(total) =
                            range.rsplit('/').next().and_then(|t| t.parse::<u64>().ok())
                        {
                            info.size = Some(total);
                            info.accept_ranges = true;
                        }
                    }
                }
                return Ok(info);
            }
            ProxyAuthScheme::Basic => {
                let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) else {
                    return Err(EngineError::InvalidState(
                        "The proxy needs Basic authentication — set the proxy username and password".to_string(),
                    ));
                };
                let resp = if ranged {
                    apply_auth(client.get(url), auth)
                        .header(RANGE, "bytes=0-0")
                        .header("proxy-authorization", basic_proxy_auth_header(user, pass))
                        .send()
                        .await?
                } else {
                    apply_auth(client.head(url), auth)
                        .header("proxy-authorization", basic_proxy_auth_header(user, pass))
                        .send()
                        .await?
                };
                let status = resp.status();
                if status.as_u16() == 407 {
                    last_err = Some(EngineError::InvalidState(
                        "Proxy authentication failed (407) — check the proxy username and password"
                            .to_string(),
                    ));
                    continue;
                }
                if status.is_success() || status == StatusCode::PARTIAL_CONTENT {
                    let mut info = from_headers(resp.headers(), resp.url().as_str())?;
                    if ranged {
                        apply_ranged_total(&mut info, resp.headers());
                    }
                    return Ok(info);
                }
                return Err(EngineError::Status(
                    status.as_u16(),
                    format!("{status} while checking the address"),
                ));
            }
        }
    }
    Err(last_err.unwrap_or_else(plain_407))
}

/// Build a `ProbeInfo` from raw `(name, value)` headers (the NTLM
/// path has no reqwest `HeaderMap` to reuse `from_headers` with).
fn probe_info_from_raw(headers: &[(String, String)], final_url: &str) -> Result<ProbeInfo> {
    let get = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let info = ProbeInfo {
        size: get("content-length").and_then(|v| v.parse::<u64>().ok()),
        accept_ranges: get("accept-ranges")
            .map(|v| v.eq_ignore_ascii_case("bytes"))
            .unwrap_or(false),
        etag: get("etag").map(str::to_string),
        last_modified: get("last-modified").map(str::to_string),
        filename: get("content-disposition").and_then(parse_content_disposition_filename),
        content_type: get("content-type").map(str::to_string),
        final_url: final_url.to_string(),
    };
    // Round 87.2 — same refusal as `from_headers`.
    info.refuse_html_page()?;
    Ok(info)
}

/// Attach a Basic-auth credential to one request builder, if any.
fn apply_auth(req: reqwest::RequestBuilder, auth: Option<(&str, &str)>) -> reqwest::RequestBuilder {
    match auth {
        Some((user, pass)) => req.basic_auth(user, Some(pass)),
        None => req,
    }
}

async fn probe_with_ranged_get(
    client: &Client,
    url: &str,
    auth: Option<(&str, &str)>,
    proxy: Option<&ProxyConfig>,
) -> Result<ProbeInfo> {
    let resp = apply_auth(client.get(url), auth)
        .header(RANGE, "bytes=0-0")
        .send()
        .await?;
    let status = resp.status();
    if status.as_u16() == 407 {
        return probe_proxy_auth_fallback(client, url, auth, proxy, true).await;
    }
    if !status.is_success() && status != StatusCode::PARTIAL_CONTENT {
        return Err(EngineError::Status(
            status.as_u16(),
            format!("{status} while checking the address"),
        ));
    }
    let mut info = from_headers(resp.headers(), resp.url().as_str())?;
    // A 206 answer carries the total in Content-Range: "bytes 0-0/<total>".
    if let Some(range) = resp
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(total) = range.rsplit('/').next().and_then(|t| t.parse::<u64>().ok()) {
            info.size = Some(total);
            info.accept_ranges = true;
        }
    }
    Ok(info)
}

fn from_headers(headers: &reqwest::header::HeaderMap, final_url: &str) -> Result<ProbeInfo> {
    let size = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let accept_ranges = headers
        .get(ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("bytes"))
        .unwrap_or(false);
    let etag = headers
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let last_modified = headers
        .get(LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let filename = headers
        .get(CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_disposition_filename);
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let info = ProbeInfo {
        size,
        accept_ranges,
        etag,
        last_modified,
        filename,
        content_type,
        final_url: final_url.to_string(),
    };
    // Round 87.2 — a probed web page is not a file: refuse it
    // here, in the one place every probe path funnels through.
    info.refuse_html_page()?;
    Ok(info)
}

/// `attachment; filename="report.pdf"` -> `report.pdf`.
/// Also handles RFC 5987 `filename*=UTF-8''...`.
fn parse_content_disposition_filename(value: &str) -> Option<String> {
    for part in value.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename*=") {
            let rest = rest.trim().trim_matches('"');
            if let Some(encoded) = rest.splitn(3, '\'').nth(2) {
                return percent_decode(encoded).filter(|s| !s.is_empty());
            }
        }
        if let Some(rest) = part.strip_prefix("filename=") {
            let name = rest.trim().trim_matches('"').trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_page_refused_for_plain_urls() {
        assert!(is_html_page_response(
            Some("text/html"),
            "https://cinejoy.pk/watch/tv/138501/1/1"
        ));
        assert!(is_html_page_response(
            Some("text/html; charset=utf-8"),
            "https://example.com/page"
        ));
        assert!(is_html_page_response(
            Some("application/xhtml+xml"),
            "https://example.com/feed"
        ));
        assert!(is_html_page_response(
            Some("TEXT/HTML"),
            "https://example.com/x"
        ));
    }

    #[test]
    fn html_page_allowed_for_explicit_page_urls_and_other_types() {
        assert!(!is_html_page_response(
            Some("text/html"),
            "https://example.com/index.html"
        ));
        assert!(!is_html_page_response(
            Some("text/html"),
            "https://example.com/a/b.htm"
        ));
        assert!(!is_html_page_response(
            Some("text/html"),
            "https://example.com/a.MHTML"
        ));
        assert!(!is_html_page_response(
            Some("text/html"),
            "https://example.com/index.html?x=1"
        ));
        assert!(!is_html_page_response(
            Some("video/mp4"),
            "https://example.com/v.mp4"
        ));
        assert!(!is_html_page_response(
            Some("application/octet-stream"),
            "https://example.com/f.bin"
        ));
        assert!(!is_html_page_response(None, "https://example.com/page"));
    }

    #[test]
    fn refuse_html_page_errors_with_the_clear_message() {
        let info = ProbeInfo {
            size: Some(5600),
            accept_ranges: false,
            etag: None,
            last_modified: None,
            filename: Some("1".to_string()),
            content_type: Some("text/html".to_string()),
            final_url: "https://cinejoy.pk/watch/tv/138501/1/1".to_string(),
        };
        let err = info.refuse_html_page().unwrap_err();
        assert_eq!(err.to_string(), HTML_PAGE_REFUSAL);
    }

    #[test]
    fn refuse_html_page_passes_normal_files() {
        let info = ProbeInfo {
            size: Some(20_971_520),
            accept_ranges: true,
            etag: None,
            last_modified: None,
            filename: Some("slow-a.mp4".to_string()),
            content_type: Some("video/mp4".to_string()),
            final_url: "http://127.0.0.1:55443/slow-a.mp4".to_string(),
        };
        assert!(info.refuse_html_page().is_ok());
    }
}
