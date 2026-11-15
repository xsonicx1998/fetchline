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

impl std::error::Error for EngineError {}

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
