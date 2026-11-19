//! Site grabber (S06 / F06) — a polite crawler.
//!
//! Rules (per `replica/recon.md` F06 and `replica/architecture.md`):
//! - Same host only: off-site links are never followed and never
//!   matched as files.
//! - Depth limit: default 2, maximum 5 — deeper values are rejected
//!   with a clear error, never silently clamped.
//! - File-type filter: at least one of the six categories (images,
//!   video, audio, documents, archives, programs) must be ticked.
//!   The safe default is images + documents only — executables
//!   (Programs) are never downloaded unless the user ticks them.
//! - Page cap: default 200 pages, hard cap 1000 — the crawl stops
//!   cleanly at the cap, never forever.
//! - Polite: at least 500 ms between page fetches, and robots.txt
//!   is fetched and honoured for the Fetchline user-agent (and `*`).
//! - Per file, 2 attempts and then the grabber gives up: the failure
//!   is recorded per file, never retried forever.
//! - No cookies are ever sent, so a page that needs a login is
//!   reported as blocked, not silently skipped or bypassed.
//! - Excluded sites (settings) are refused before any crawling.
//!
//! This module holds only pure rules and types — no HTTP, no SQLite
//! — so the rules are unit-testable without a server. The crawl
//! itself lives in `manager.rs` and reuses `links::extract_links`
//! (the S07 page-link extraction) and the single `add_download`
//! path, so the duplicate guard, category auto-assign, queue filing,
//! proxy and speed limiter all apply to grabbed files.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{EngineError, Result};
use crate::links::LinkKind;
use crate::timeutil::parse_iso_to_unix;

/// User-agent the crawler announces itself with, in robots.txt and
/// in HTTP requests (the shared reqwest client already sends
/// `Fetchline/0.1`; this is the name robots.txt groups match on).
pub const GRABBER_USER_AGENT: &str = "Fetchline";

/// The seeded main queue (`replica/schema.sql`): where grabbed
/// files go when the project names no other queue.
pub const MAIN_QUEUE_ID: &str = "queue-main";

/// Default explore depth (pages below the start page).
pub const DEFAULT_DEPTH: i64 = 2;
/// Maximum explore depth — deeper values are rejected, not clamped.
pub const MAX_DEPTH: i64 = 5;
/// Default page cap.
pub const DEFAULT_MAX_PAGES: i64 = 200;
/// Hard page cap — the crawl stops cleanly here, never forever.
pub const MAX_PAGES_HARD: i64 = 1000;
/// Minimum delay between page fetches. A hard rule, not a hint.
pub const POLITE_DELAY: Duration = Duration::from_millis(500);
/// Attempts per file before the grabber gives up on it (recon F06).
pub const MAX_FILE_ATTEMPTS: u32 = 2;
/// Largest page the crawler will read (same cap as S07).
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

/// The six file-type categories the wizard offers. These reuse the
/// `file_types_auto` categories from settings — one vocabulary for
/// the whole app, not a second list to drift.
pub const KNOWN_TYPES: [&str; 6] = ["image", "video", "audio", "document", "archive", "program"];

/// Safe default filter: images + documents only. Programs
/// (executables) are never downloaded unless the user explicitly
/// ticks them in the wizard.
pub const SAFE_DEFAULT_TYPES: [&str; 2] = ["image", "document"];

/// The wizard's filter set, stored as JSON in
/// `grabber_projects.filters_json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrabberFilters {
    /// File-type categories to match, e.g. `["image", "document"]`.
    #[serde(default)]
    pub types: Vec<String>,
    /// Explore depth: 0 = the start page only.
    #[serde(default = "default_depth")]
    pub depth: i64,
    /// Maximum pages to explore before stopping cleanly.
    #[serde(default = "default_max_pages")]
    pub max_pages: i64,
}

fn default_depth() -> i64 {
    DEFAULT_DEPTH
}

fn default_queue_id() -> String {
    MAIN_QUEUE_ID.to_string()
}

fn default_max_pages() -> i64 {
    DEFAULT_MAX_PAGES
}

impl Default for GrabberFilters {
    fn default() -> Self {
        Self {
            types: SAFE_DEFAULT_TYPES.iter().map(|s| s.to_string()).collect(),
            depth: DEFAULT_DEPTH,
            max_pages: DEFAULT_MAX_PAGES,
        }
    }
}

/// One file the crawl matched, as the progress view lists it.
/// `status` is `matched` (found, not downloaded yet),
/// `downloaded` or `failed` (with `error` saying why).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedFile {
    pub url: String,
    pub filename: String,
    /// Lowercase kind label (`image`, `video`, … — see `LinkKind`).
    pub kind: String,
    pub size: Option<u64>,
    pub status: String,
    pub error: Option<String>,
    pub download_id: Option<String>,
}

/// One site-grabber project, as the UI sees it. `schedule_start_at`
/// is the optional one-time start time (UTC ISO-8601, reusing the
/// existing scheduler — there is no second scheduler). `queue_id`
/// is the queue grabbed files are filed into (the main queue when
/// the project names none — Milestone 4); `on_complete_action` /
/// `on_complete_program` are the EXISTING queue completion actions
/// (none | open_file | run_program | shutdown | hibernate), armed
/// on the target queue through the existing `PendingAction` +
/// 60 s countdown when — and only when — the run actually
/// downloaded at least one file (the BUG-003 lesson).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrabberProjectRow {
    pub id: String,
    pub name: String,
    pub start_url: String,
    pub filters: GrabberFilters,
    /// `explore` lists matched files without downloading;
    /// `download` downloads them.
    pub mode: String,
    pub schedule_start_at: Option<String>,
    /// Queue grabbed files go to; `queue-main` when unspecified.
    #[serde(default = "default_queue_id")]
    pub queue_id: String,
    /// Completion action for the project (see above).
    #[serde(default)]
    pub on_complete_action: String,
    /// Program to run for a `run_program` action, if any.
    #[serde(default)]
    pub on_complete_program: Option<String>,
    /// idle | exploring | downloading | done | failed | stopped
    pub status: String,
    pub pages_explored: i64,
    pub files_matched: i64,
    pub files_downloaded: i64,
    pub files_failed: i64,
    pub error_message: Option<String>,
    pub matched_files: Vec<MatchedFile>,
    pub created_at: String,
}

/// Validate a new project's inputs and normalise them into filters.
/// Every rejection names the problem — nothing is silently clamped.
pub fn validate_grabber_input(
    name: &str,
    start_url: &str,
    types: &[String],
    depth: i64,
    max_pages: i64,
    mode: &str,
    schedule_start_at: Option<&str>,
) -> Result<GrabberFilters> {
    if name.trim().is_empty() {
        return Err(EngineError::InvalidState(
            "Give the project a name".to_string(),
        ));
    }
    let parsed = reqwest::Url::parse(start_url.trim())
        .map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(EngineError::InvalidUrl(
            "The start address must start with http:// or https://".to_string(),
        ));
    }
    if parsed.host_str().is_none() {
        return Err(EngineError::InvalidUrl(
            "The start address has no host".to_string(),
        ));
    }
    if types.is_empty() {
        return Err(EngineError::InvalidState(
            "Pick at least one file type to grab".to_string(),
        ));
    }
    let mut clean: Vec<String> = Vec::new();
    for t in types {
        let t = t.trim().to_ascii_lowercase();
        if t.is_empty() {
            continue;
        }
        if !KNOWN_TYPES.contains(&t.as_str()) {
            return Err(EngineError::InvalidState(format!("Unknown file type: {t}")));
        }
        if !clean.contains(&t) {
            clean.push(t);
        }
    }
    if clean.is_empty() {
        return Err(EngineError::InvalidState(
            "Pick at least one file type to grab".to_string(),
        ));
    }
    if !(0..=MAX_DEPTH).contains(&depth) {
        return Err(EngineError::InvalidState(format!(
            "Explore depth must be between 0 and {MAX_DEPTH} — {depth} is too deep"
        )));
    }
    if !(1..=MAX_PAGES_HARD).contains(&max_pages) {
        return Err(EngineError::InvalidState(format!(
            "The page limit must be between 1 and {MAX_PAGES_HARD} — {max_pages} would crawl too far"
        )));
    }
    if !matches!(mode, "explore" | "download") {
        return Err(EngineError::InvalidState(format!(
            "Unknown grabber mode: {mode}"
        )));
    }
    if let Some(start) = schedule_start_at.map(str::trim).filter(|s| !s.is_empty()) {
        if parse_iso_to_unix(start).is_none() {
            return Err(EngineError::InvalidState(
                "The scheduled start time is not a valid date and time — use ISO format, e.g. 2026-11-14T20:30:00Z"
                    .to_string(),
            ));
        }
    }
    Ok(GrabberFilters {
        types: clean,
        depth,
        max_pages,
    })
}

/// True when `url` points at a page the crawler may follow (an HTML
/// page, a directory, or a bare host), rather than a file. The
/// extension decides: known page extensions and no extension at all
/// are pages; anything else is a file of some kind.
pub fn is_page_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let path = parsed.path();
    if path.ends_with('/') {
        return true;
    }
    let last = path.rsplit('/').next().unwrap_or("");
    match last.rsplit('.').next() {
        Some(ext) if last.contains('.') => matches!(
            ext.to_ascii_lowercase().as_str(),
            "html" | "htm" | "php" | "asp" | "aspx" | "jsp" | "shtml"
        ),
        _ => true, // no extension: a directory-style page
    }
}

/// True when two URLs share a host (case-insensitive). The crawl
/// never follows a link whose host differs — scheme and port do not
/// matter, the host is the boundary.
pub fn same_host(a: &str, b: &str) -> bool {
    let host = |u: &str| {
        reqwest::Url::parse(u)
            .ok()
            .and_then(|p| p.host_str().map(|h| h.to_ascii_lowercase()))
    };
    match (host(a), host(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// True when a link's kind is one of the selected file types.
pub fn type_matches(kind: LinkKind, types: &[String]) -> bool {
    let label = kind.label().to_ascii_lowercase();
    types.iter().any(|t| t == &label)
}

/// Parse a robots.txt body and say whether `path` is disallowed for
/// the Fetchline crawler. Groups whose User-agent is `Fetchline` or
/// `*` apply; a matching `Allow` beats a `Disallow`, and an empty
/// `Disallow:` allows everything. A missing or unparsable body
/// allows everything — a site that cannot be read is not assumed
/// to forbid crawling, but the start URL is still checked against
/// whatever was actually fetched.
///
/// The Allow-beats-Disallow decision is made across ALL applying
/// groups, not per group: a matching `Allow` in the Fetchline group
/// beats a matching `Disallow` in the `*` group whichever group
/// comes first in the file (BUG-017 — the old code returned on the
/// first applying group, so a `*` group written first hid the
/// Fetchline group's Allow).
pub fn robots_disallows(robots_txt: &str, path: &str) -> bool {
    // Parse groups, keeping each group's user-agents with its rules.
    let mut groups: Vec<(Vec<String>, Vec<String>, Vec<String>)> = Vec::new();
    let mut ua: Vec<String> = Vec::new();
    let mut allows: Vec<String> = Vec::new();
    let mut disallows: Vec<String> = Vec::new();
    let mut started = false;
    for raw in robots_txt.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if key == "user-agent" {
            if started {
                groups.push((
                    std::mem::take(&mut ua),
                    std::mem::take(&mut allows),
                    std::mem::take(&mut disallows),
                ));
            }
            ua.push(value.to_ascii_lowercase());
            started = true;
        } else if key == "allow" && started {
            allows.push(value);
        } else if key == "disallow" && started && !value.is_empty() {
            disallows.push(value);
        }
    }
    if started {
        groups.push((ua, allows, disallows));
    }
    let ua_name = GRABBER_USER_AGENT.to_ascii_lowercase();
    // Collect the rules of every applying group, then decide once:
    // a matching Allow in ANY applying group (in particular the
    // Fetchline group) beats a matching Disallow in any other
    // (in particular the `*` group), whichever group comes first
    // in the file. Returning on the first applying group made the
    // result depend on group order (BUG-017).
    let mut allowed = false;
    let mut disallowed = false;
    for (uas, allows, disallows) in &groups {
        let applies = uas.iter().any(|u| u == "*" || u == &ua_name);
        if !applies {
            continue;
        }
        if allows
            .iter()
            .any(|a| !a.is_empty() && path.starts_with(a.as_str()))
        {
            allowed = true;
        }
        if disallows.iter().any(|d| path.starts_with(d.as_str())) {
            disallowed = true;
        }
    }
    disallowed && !allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robots_blocks_prefix_and_all() {
        let txt = "User-agent: *\nDisallow: /private/\nDisallow: /tmp\n";
        assert!(robots_disallows(txt, "/private/page.html"));
        assert!(robots_disallows(txt, "/tmpfile"));
        assert!(!robots_disallows(txt, "/public/page.html"));
        assert!(robots_disallows(
            "User-agent: *\nDisallow: /\n",
            "/anything"
        ));
        assert!(!robots_disallows("User-agent: *\nDisallow:\n", "/anything"));
    }

    #[test]
    fn robots_allow_beats_disallow_and_other_agents_ignored() {
        let txt = "User-agent: OtherBot\nDisallow: /\n\nUser-agent: Fetchline\nAllow: /open/\nDisallow: /\n";
        assert!(!robots_disallows(txt, "/open/page.html"));
        assert!(robots_disallows(txt, "/closed/page.html"));
        assert!(!robots_disallows(
            "User-agent: Googlebot\nDisallow: /\n",
            "/x"
        ));
    }

    #[test]
    fn page_detection_and_same_host() {
        assert!(is_page_url("http://127.0.0.1/index.html"));
        assert!(is_page_url("http://127.0.0.1/gallery/"));
        assert!(is_page_url("http://127.0.0.1/gallery"));
        assert!(!is_page_url("http://127.0.0.1/pic.png"));
        assert!(!is_page_url("http://127.0.0.1/doc.pdf"));
        assert!(same_host("http://127.0.0.1/a", "http://127.0.0.1:8080/b"));
        assert!(!same_host("http://127.0.0.1/a", "http://localhost:8080/b"));
        assert!(!same_host(
            "http://a.example.com/x",
            "http://b.example.com/x"
        ));
    }

    #[test]
    fn validation_rejects_depth_pages_and_empty_types() {
        let err = validate_grabber_input(
            "p",
            "http://127.0.0.1/",
            &["image".into()],
            6,
            200,
            "download",
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("too deep"));
        assert!(
            validate_grabber_input("p", "http://127.0.0.1/", &[], 2, 200, "download", None)
                .is_err()
        );
        assert!(validate_grabber_input(
            "p",
            "http://127.0.0.1/",
            &["image".into()],
            2,
            1001,
            "download",
            None
        )
        .is_err());
        assert!(validate_grabber_input(
            "p",
            "not a url",
            &["image".into()],
            2,
            200,
            "download",
            None
        )
        .is_err());
        assert!(validate_grabber_input(
            "p",
            "http://127.0.0.1/",
            &["exe".into()],
            2,
            200,
            "download",
            None
        )
        .is_err());
        let ok = validate_grabber_input(
            "p",
            "http://127.0.0.1/",
            &["Image".into(), "image".into()],
            2,
            200,
            "download",
            None,
        )
        .unwrap();
        assert_eq!(ok.types, vec!["image".to_string()]);
    }
}
