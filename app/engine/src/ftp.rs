//! Minimal FTP transport (S09 Proxy row 18).
//!
//! What is supported, honestly:
//! - Direct FTP: control connection, `USER`/`PASS` login (credentials
//!   from the URL, else anonymous), `TYPE I`, `SIZE` for the probe,
//!   `REST` + `RETR` over passive mode (`PASV`) for downloads, with
//!   resume from an arbitrary byte offset where the server supports
//!   `REST`. Files are downloaded through the engine's normal segment
//!   machinery, so pause/resume, the speed limiter and the retry
//!   backoff all apply exactly as for HTTP.
//! - FTP proxy: when the configured proxy is type `ftp`, the engine
//!   connects to the proxy host instead of the origin and logs in
//!   with the proxy credentials first (`USER`/`PASS`), then with
//!   `USER <ftp-user>@<ftp-host>` / `PASS <ftp-password>` — the
//!   widely used `user@host` FTP-proxy convention — after which
//!   `SIZE`/`PASV`/`RETR` run as normal. The proxy's bypass list
//!   applies (a bypassed FTP host is fetched direct). The fixture
//!   FTP proxy in `tests/proxy_tests.rs` implements exactly this
//!   login sequence and serves files itself, which proves the wire
//!   format (both credential sets, in order, and the `user@host`
//!   form); a full relay proxy that opens its own upstream
//!   connection is the proxy server's job, not the client's.
//! - FTP through an HTTP proxy is OUT of scope and is refused with
//!   a clear error: tunnelling FTP over HTTP is a different
//!   protocol, not a flag.
//! - Active mode (`PORT`) is not implemented: passive mode is what
//!   modern servers and NAT expect, and it is the only mode the
//!   fixture (and the engine) speaks.
//! - No TLS (FTPS): plain FTP only. A server that demands TLS
//!   answers the login with an error, which the engine surfaces.
//!
//! The password is never logged: commands are written without ever
//! printing them, and replies are checked by code only.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::{EngineError, Result};
use crate::probe::ProbeInfo;
use crate::proxy::ProxyConfig;

const FTP_TIMEOUT: Duration = Duration::from_secs(30);
const FTP_CHUNK: usize = 65536;

/// A parsed `ftp://` URL.
#[derive(Debug, Clone)]
pub struct FtpTarget {
    pub host: String,
    pub port: u16,
    /// Login user (URL userinfo, else `anonymous`).
    pub user: String,
    /// Login password (URL userinfo, else `anonymous@`).
    pub pass: String,
    /// Path as sent to the server (leading `/` kept).
    pub path: String,
}

/// Parse an `ftp://` URL. `user:pass@host:port/path` are all
/// optional except the host and a non-empty path.
pub fn parse_ftp_url(url: &str) -> Result<FtpTarget> {
    let parsed = reqwest::Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
    if parsed.scheme() != "ftp" {
        return Err(EngineError::InvalidUrl(format!("Not an ftp:// URL: {url}")));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| EngineError::InvalidUrl("The FTP address has no host".to_string()))?
        .to_string();
    let path = parsed.path().to_string();
    if path.is_empty() || path == "/" {
        return Err(EngineError::InvalidUrl(
            "The FTP address has no file path".to_string(),
        ));
    }
    Ok(FtpTarget {
        host,
        port: parsed.port().unwrap_or(21),
        user: percent_decode(parsed.username()).unwrap_or_else(|| "anonymous".to_string()),
        pass: percent_decode(parsed.password().unwrap_or(""))
            .unwrap_or_else(|| "anonymous@".to_string()),
        path,
    })
}

fn percent_decode(input: &str) -> Option<String> {
    if !input.contains('%') {
        return if input.is_empty() {
            None
        } else {
            Some(input.to_string())
        };
    }
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

/// The filename of an FTP path (last segment, decoded).
pub fn ftp_filename(path: &str) -> Option<String> {
    path.rsplit('/')
        .next()
        .and_then(|f| percent_decode(f))
        .filter(|f| !f.is_empty())
}

/// Which proxy, if any, an FTP download to `host` must use: the
/// configured proxy only when it is type `ftp` and the host is not
/// on its bypass list. An HTTP or SOCKS proxy never applies to FTP
/// (FTP through an HTTP proxy is out of scope — see the module
/// docs), and a bypassed host is fetched direct.
pub fn ftp_proxy_for<'a>(host: &str, proxy: Option<&'a ProxyConfig>) -> Option<&'a ProxyConfig> {
    let cfg = proxy?;
    if cfg.proxy_type != "ftp" {
        return None;
    }
    if crate::proxy::host_bypasses_proxy(host, &cfg.bypass) {
        return None;
    }
    Some(cfg)
}

// ---------------------------------------------------------------------------
// FTP control connection
// ---------------------------------------------------------------------------

struct FtpClient {
    control: BufReader<TcpStream>,
}

impl FtpClient {
    /// Connect to `host:port` (origin, or the FTP proxy) and read
    /// the 220 greeting.
    async fn connect(host: &str, port: u16) -> Result<Self> {
        let stream = tokio::time::timeout(FTP_TIMEOUT, TcpStream::connect((host, port)))
            .await
            .map_err(|_| {
                EngineError::InvalidState(format!("Timed out connecting to {host}:{port}"))
            })??;
        let mut client = Self {
            control: BufReader::new(stream),
        };
        let (code, _) = client.read_reply().await?;
        if code != 220 {
            return Err(EngineError::InvalidState(format!(
                "The FTP server did not greet us (got {code})"
            )));
        }
        Ok(client)
    }

    /// Read one reply: `NNN text`, with `NNN-` multi-line replies
    /// read until the line `NNN ` (same code, space).
    async fn read_reply(&mut self) -> Result<(u16, String)> {
        let work = async {
            let mut first = String::new();
            self.control.read_line(&mut first).await?;
            let code: u16 = first
                .get(0..3)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| {
                    EngineError::InvalidState(format!(
                        "The FTP server answered with a bad reply: {first}"
                    ))
                })?;
            let mut text = first.trim_end().to_string();
            if first.as_bytes().get(3) == Some(&b'-') {
                // Multi-line: keep reading until "NNN ".
                loop {
                    let mut line = String::new();
                    self.control.read_line(&mut line).await?;
                    text.push('\n');
                    text.push_str(line.trim_end());
                    if line.len() >= 4
                        && line[..3].parse::<u16>().map(|c| c == code).unwrap_or(false)
                        && line.as_bytes()[3] == b' '
                    {
                        break;
                    }
                }
            }
            Ok::<_, EngineError>((code, text))
        };
        tokio::time::timeout(FTP_TIMEOUT, work).await.map_err(|_| {
            EngineError::InvalidState("The FTP server did not answer in time".to_string())
        })?
    }

    async fn command(&mut self, cmd: &str) -> Result<(u16, String)> {
        // Never log the command: it may carry a password.
        self.control
            .write_all(format!("{cmd}\r\n").as_bytes())
            .await?;
        self.control.flush().await?;
        self.read_reply().await
    }

    /// Log in. With an FTP proxy: proxy credentials first, then
    /// `USER <ftp-user>@<ftp-host>`; direct: `USER <ftp-user>`.
    /// Returns an error (never a partial login) on any refusal.
    async fn login(&mut self, target: &FtpTarget, proxy: Option<&ProxyConfig>) -> Result<()> {
        if let Some(cfg) = proxy {
            if let Some(user) = &cfg.username {
                if !user.trim().is_empty() {
                    let (code, _) = self.command(&format!("USER {user}")).await?;
                    if code == 331 {
                        let pass = cfg.password.as_deref().unwrap_or("");
                        let (code, _) = self.command(&format!("PASS {pass}")).await?;
                        if code != 230 {
                            return Err(EngineError::InvalidState(format!(
                                "The FTP proxy refused the login (got {code}) — check the proxy username and password"
                            )));
                        }
                    } else if code != 230 {
                        return Err(EngineError::InvalidState(format!(
                            "The FTP proxy refused the login (got {code}) — check the proxy username and password"
                        )));
                    }
                }
            }
            // Origin login through the proxy, user@host form.
            let (code, _) = self
                .command(&format!("USER {}@{}", target.user, target.host))
                .await?;
            if code == 331 {
                let (code, _) = self.command(&format!("PASS {}", target.pass)).await?;
                if code != 230 {
                    return Err(EngineError::InvalidState(format!(
                        "The FTP server refused the login (got {code})"
                    )));
                }
            } else if code != 230 {
                return Err(EngineError::InvalidState(format!(
                    "The FTP server refused the login (got {code})"
                )));
            }
            return Ok(());
        }
        let (code, _) = self.command(&format!("USER {}", target.user)).await?;
        if code == 331 {
            let (code, _) = self.command(&format!("PASS {}", target.pass)).await?;
            if code != 230 {
                return Err(EngineError::InvalidState(format!(
                    "The FTP server refused the login (got {code})"
                )));
            }
        } else if code != 230 {
            return Err(EngineError::InvalidState(format!(
                "The FTP server refused the login (got {code})"
            )));
        }
        Ok(())
    }

    async fn type_binary(&mut self) -> Result<()> {
        let (code, _) = self.command("TYPE I").await?;
        if code != 200 {
            return Err(EngineError::InvalidState(format!(
                "The FTP server refused binary mode (got {code})"
            )));
        }
        Ok(())
    }

    /// `SIZE path` -> `Some(bytes)`, or `None` when the server does
    /// not support it (550/500) — the file is then downloaded
    /// open-ended, as a single segment.
    async fn size(&mut self, path: &str) -> Result<Option<u64>> {
        let (code, text) = self.command(&format!("SIZE {path}")).await?;
        if code == 213 {
            let size = text
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok());
            return Ok(size);
        }
        Ok(None)
    }

    /// `PASV` -> the data address the server listens on.
    async fn pasv(&mut self) -> Result<(String, u16)> {
        let (code, text) = self.command("PASV").await?;
        if code != 227 {
            return Err(EngineError::InvalidState(format!(
                "The FTP server refused passive mode (got {code})"
            )));
        }
        let start = text.find('(').ok_or_else(|| {
            EngineError::InvalidState("The FTP server sent a bad PASV reply".to_string())
        })?;
        let end = text[start..].find(')').ok_or_else(|| {
            EngineError::InvalidState("The FTP server sent a bad PASV reply".to_string())
        })? + start;
        let parts: Vec<u16> = text[start + 1..end]
            .split(',')
            .map(|p| p.trim().parse::<u16>())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| {
                EngineError::InvalidState("The FTP server sent a bad PASV reply".to_string())
            })?;
        if parts.len() != 6 {
            return Err(EngineError::InvalidState(
                "The FTP server sent a bad PASV reply".to_string(),
            ));
        }
        let host = format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3]);
        let port = parts[4] * 256 + parts[5];
        Ok((host, port))
    }

    async fn quit(&mut self) {
        let _ = self.command("QUIT").await;
    }
}

/// Connect (direct, or via the FTP proxy) and log in, in binary mode.
async fn connect_and_login(
    url: &str,
    proxy: Option<&ProxyConfig>,
) -> Result<(FtpClient, FtpTarget)> {
    let target = parse_ftp_url(url)?;
    let via = ftp_proxy_for(&target.host, proxy);
    let (host, port) = match via {
        Some(cfg) => (cfg.host.clone(), cfg.port as u16),
        None => (target.host.clone(), target.port),
    };
    let mut client = FtpClient::connect(&host, port).await?;
    if let Err(e) = client.login(&target, via).await {
        client.quit().await;
        return Err(e);
    }
    if let Err(e) = client.type_binary().await {
        client.quit().await;
        return Err(e);
    }
    Ok((client, target))
}

/// Probe an FTP URL: `SIZE` for the size, nothing else. `accept_ranges`
/// is true when the size is known (the server is then assumed to
/// support `REST`; a server that does not fails the download with a
/// clear error instead of stitching wrong bytes).
pub async fn ftp_probe(url: &str, proxy: Option<&ProxyConfig>) -> Result<ProbeInfo> {
    let (mut client, target) = connect_and_login(url, proxy).await?;
    let size = client.size(&target.path).await;
    client.quit().await;
    let size = size?;
    Ok(ProbeInfo {
        size,
        accept_ranges: size.is_some(),
        etag: None,
        last_modified: None,
        filename: ftp_filename(&target.path),
        final_url: url.to_string(),
    })
}

// ---------------------------------------------------------------------------
// RETR body reader
// ---------------------------------------------------------------------------

/// One FTP download: the data connection, read in chunks. The
/// control connection is kept so `finish` can read the closing
/// 226 reply and confirm the transfer completed.
pub struct FtpBodyReader {
    data: TcpStream,
    control: FtpClient,
    remaining: Option<u64>,
    done: bool,
    /// The reader stopped at the segment's end while the server was
    /// still sending the rest of the file (a split segment): the
    /// transfer was cut short on purpose, so `finish` must not
    /// demand a 226 for it.
    stopped_early: bool,
}

impl FtpBodyReader {
    /// Read up to `buf.len()` bytes. Returns 0 at EOF.
    pub async fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let limit = match self.remaining {
            Some(r) => (r as usize).min(buf.len()),
            None => buf.len(),
        };
        if limit == 0 {
            self.done = true;
            return Ok(0);
        }
        let n = self.data.read(&mut buf[..limit]).await?;
        if n == 0 {
            self.done = true;
            return Ok(0);
        }
        if let Some(r) = &mut self.remaining {
            *r -= n as u64;
            if *r == 0 {
                self.done = true;
                self.stopped_early = true;
            }
        }
        Ok(n)
    }

    /// End the transfer: close the data connection, then read the
    /// control reply — 226 (or 250) means the server confirms the
    /// file arrived whole. Anything else is a failed download, so
    /// the segment retries from its checkpoint instead of the
    /// engine trusting a truncated file. A segment stopped early
    /// (split end) skips the reply check: the server was still
    /// sending when the data connection closed.
    pub async fn finish(mut self) -> Result<()> {
        drop(self.data);
        if self.stopped_early {
            self.control.quit().await;
            return Ok(());
        }
        let (code, _) = self.control.read_reply().await?;
        if code != 226 && code != 250 {
            return Err(EngineError::InvalidState(format!(
                "The FTP transfer did not finish cleanly (got {code})"
            )));
        }
        self.control.quit().await;
        Ok(())
    }
}

/// Start a `RETR` for bytes `start..=end` (`end = None` reads to
/// EOF). The caller pumps `read_chunk` and must call `finish`.
pub async fn ftp_retr(
    url: &str,
    proxy: Option<&ProxyConfig>,
    start: u64,
    end: Option<u64>,
) -> Result<FtpBodyReader> {
    let (mut client, target) = connect_and_login(url, proxy).await?;
    if start > 0 {
        let (code, _) = client.command(&format!("REST {start}")).await?;
        if code != 350 {
            client.quit().await;
            return Err(EngineError::InvalidState(format!(
                "The FTP server does not support resume (REST got {code})"
            )));
        }
    }
    let (host, port) = match client.pasv().await {
        Ok(v) => v,
        Err(e) => {
            client.quit().await;
            return Err(e);
        }
    };
    let data =
        match tokio::time::timeout(FTP_TIMEOUT, TcpStream::connect((host.as_str(), port))).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                client.quit().await;
                return Err(EngineError::from(e));
            }
            Err(_) => {
                client.quit().await;
                return Err(EngineError::InvalidState(
                    "Timed out connecting to the FTP data port".to_string(),
                ));
            }
        };
    let (code, _) = client.command(&format!("RETR {}", target.path)).await?;
    if code != 150 && code != 125 {
        client.quit().await;
        return Err(EngineError::Status(
            code,
            format!("FTP RETR failed ({code})"),
        ));
    }
    let remaining = end.map(|e| e.saturating_sub(start) + 1);
    Ok(FtpBodyReader {
        data,
        control: client,
        remaining,
        done: false,
        stopped_early: false,
    })
}

/// How many bytes a chunk read may take (mirrors the HTTP segment
/// pump's 64 KiB reads).
pub const FTP_CHUNK_SIZE: usize = FTP_CHUNK;
