use std::fmt;

/// Errors the engine can return. Messages are user-facing: they are shown
/// in the download table / progress dialog as the error reason.
#[derive(Debug)]
pub enum EngineError {
    /// The URL is not usable (bad scheme, unparsable, empty).
    InvalidUrl(String),
    /// HTTP transport or protocol failure.
    Http(reqwest::Error),
    /// The server answered with a non-success status.
    Status(u16, String),
    /// SQLite failure.
    Db(rusqlite::Error),
    /// No download with this id exists.
    NotFound(String),
    /// The operation is not valid in the download's current state.
    InvalidState(String),
    /// Filesystem failure (create, write, rename).
    Io(std::io::Error),
    /// JSON (de)serialisation failure.
    Json(serde_json::Error),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::InvalidUrl(m) => write!(f, "Invalid address: {m}"),
            EngineError::Http(e) => write!(f, "Network error: {e}"),
            EngineError::Status(code, m) => write!(f, "Server replied {code}: {m}"),
            EngineError::Db(e) => write!(f, "Database error: {e}"),
            EngineError::NotFound(id) => write!(f, "Download not found: {id}"),
            EngineError::InvalidState(m) => write!(f, "{m}"),
            EngineError::Io(e) => write!(f, "File error: {e}"),
            EngineError::Json(e) => write!(f, "Data error: {e}"),
        }
    }
}

impl std::error::Error for EngineError {
    // Round 71 (B4) — expose the inner error so callers can walk
    // the FULL chain (`error_chain_text` in manager.rs). The
    // Display text alone hides the cause: "Network error: error
    // sending request" never says whether the SYN was dropped,
    // the server reset the connection, or the TLS handshake
    // failed — the walk does.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EngineError::Http(e) => Some(e),
            EngineError::Db(e) => Some(e),
            EngineError::Io(e) => Some(e),
            EngineError::Json(e) => Some(e),
            _ => None,
        }
    }
}

/// Round 71 (B4) — the full error chain as one line, e.g.
/// "Network error: error sending request for url (...) <- client
/// error (Connect) <- ... <- A connection attempt failed ...".
/// `Display` alone stops at the first link; the chain is what
/// names the cause.
pub fn error_chain_text(e: &EngineError) -> String {
    let mut parts = vec![e.to_string()];
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(s) = src {
        parts.push(s.to_string());
        src = s.source();
    }
    parts.join(" <- ")
}

impl From<reqwest::Error> for EngineError {
    fn from(e: reqwest::Error) -> Self {
        EngineError::Http(e)
    }
}

impl From<rusqlite::Error> for EngineError {
    fn from(e: rusqlite::Error) -> Self {
        EngineError::Db(e)
    }
}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        EngineError::Io(e)
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(e: serde_json::Error) -> Self {
        EngineError::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;
