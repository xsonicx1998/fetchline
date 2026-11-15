//! Update check ("Quick update check", S09 / should-have).
//!
//! `check_for_updates()` fetches a version manifest from
//! Fetchline's own update server and reports whether a newer
//! version exists. It NEVER installs anything: the result lists
//! the new version and its changelog and the user decides —
//! there is no auto-install, and no download is started by the
//! check itself.
//!
//! Endpoint: the URL is a setting (`settings.update_url`), so the
//! user (or a future release) can point Fetchline at their own
//! release server. The default is a documented placeholder —
//! `https://updates.fetchline.app/releases/latest.json` — which
//! does not exist yet and must be replaced with the real release
//! server before shipping. The check is never pointed at IDM's
//! servers or at any other third-party update feed. Tests serve
//! the manifest from a local fixture server only.
//!
//! Manifest format (JSON):
//! ```json
//! {
//!   "version": "0.2.0",
//!   "changelog": ["Faster segmented downloads", "Fixed a crash on pause"],
//!   "download_url": "https://updates.fetchline.app/releases/fetchline-0.2.0.msi"
//! }
//! ```
//! `changelog` may also be given as `changes`, or as a single
//! `notes` string (one change per line). `download_url` is
//! optional — without it the dialog shows the version and the
//! changelog and tells the user where to get the installer.
//!
//! Install: the Tauri updater plugin is NOT configured in this
//! build (`tauri.conf.json` has no `plugins.updater` section and
//! `src-tauri/Cargo.toml` does not depend on `tauri-plugin-updater`),
//! so there is no signed release pipeline yet. Until one exists,
//! `UpdateCheckResult.updater_configured` is `false` and the
//! dialog offers the installer address to copy instead of an
//! Install button that would pretend to work. Wiring the real
//! install means: add the updater plugin, publish signed
//! releases, and point `update_url` at the server that serves
//! them — the check itself needs no change.
//!
//! Weekly check: the existing 30 s scheduler tick calls
//! `maybe_weekly_update_check` (see `manager.rs`); there is no
//! second scheduler. A check is due when `check_updates_weekly`
//! is on and the last recorded check is at least a week old (or
//! there has never been one). The last-check time is recorded
//! after every completed check — success or failure — so a
//! failed check retries next week, not on every tick. A manual
//! check always runs, even when the weekly setting is off.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::error::{EngineError, Result};
use crate::timeutil::unix_to_iso;

/// This build's version — kept in step with `Cargo.toml`,
/// `tauri.conf.json` and `package.json` (all `0.1.0`).
pub const CURRENT_VERSION: &str = "0.1.0";

/// Placeholder update endpoint. It is Fetchline's own address
/// (not IDM's, not any third-party feed) and it does not exist
/// yet: replace it with the real release server before shipping.
/// The setting (`settings.update_url`) defaults to this value.
pub const DEFAULT_UPDATE_URL: &str = "https://updates.fetchline.app/releases/latest.json";

/// Gap between automatic checks: one week.
pub const UPDATE_CHECK_INTERVAL_SECS: i64 = 7 * 24 * 60 * 60;

/// A manifest larger than this is refused, not read — a version
/// manifest is a few hundred bytes; anything bigger is not one.
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// Whether the Tauri updater plugin is configured in this build.
/// It is not (see the module docs): the check reports new
/// versions, it does not install them.
pub const UPDATER_CONFIGURED: bool = false;

/// A parsed version: dotted numbers, plus an optional
/// pre-release suffix (`1.2.3-beta.1`). Build metadata (`+…`)
/// is ignored, as in semver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    numbers: Vec<u64>,
    pre_release: Option<Vec<String>>,
}

/// Parse a version string. Accepts an optional leading `v`
/// (`v1.2.3`) and surrounding whitespace. Returns `None` for
/// anything that is not dotted numbers with an optional
/// pre-release suffix — a manifest carrying one is malformed.
pub fn parse_version(raw: &str) -> Option<Version> {
    let s = raw.trim().strip_prefix(['v', 'V']).unwrap_or(raw.trim());
    if s.is_empty() {
        return None;
    }
    // Build metadata (`+…`) is not part of the version: drop it
    // before comparing, as in semver.
    let s = s.split_once('+').map(|(head, _)| head).unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    if core.is_empty() || core.split('.').any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let numbers: Vec<u64> = core
        .split('.')
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    let pre_release = match pre {
        None => None,
        Some(p) => {
            if p.is_empty()
                || p.split('.').any(|seg| {
                    seg.is_empty()
                        || !seg
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            {
                return None;
            }
            Some(p.split('.').map(str::to_string).collect())
        }
    };
    Some(Version { numbers, pre_release })
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.numbers.len().max(other.numbers.len());
        for i in 0..len {
            let a = self.numbers.get(i).copied().unwrap_or(0);
            let b = other.numbers.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }
        // A release outranks any pre-release of the same numbers:
        // 1.0.0 > 1.0.0-beta. Two pre-releases compare segment by
        // segment: numeric segments numerically (2 < 10), a numeric
        // segment before an alphanumeric one, alphanumeric segments
        // lexically; a shorter list that is a prefix of the longer
        // one is smaller (beta < beta.1).
        match (&self.pre_release, &other.pre_release) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                for (x, y) in a.iter().zip(b.iter()) {
                    let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                        (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => x.cmp(y),
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                a.len().cmp(&b.len())
            }
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compare two version strings numerically, not as plain text —
/// `1.10.0` is newer than `1.9.9`, which a string compare gets
/// backwards. `None` when either string is not a version.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    Some(parse_version(a)?.cmp(&parse_version(b)?))
}

/// Is `latest` newer than `current`? `false` when either string
/// is not a version (the caller reports that as malformed).
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!(compare_versions(latest, current), Some(Ordering::Greater))
}

/// The version manifest, as served by the update endpoint.
/// `changelog` is the list of changes; `notes` is the same list
/// as one string (one change per line) for servers that serve it
/// that way — `parse_manifest` merges them, changelog first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    /// `#[serde(default)]` so a manifest with no `version` field
    /// reaches the explicit "no version" error in
    /// `parse_manifest` instead of a generic JSON error.
    #[serde(default)]
    pub version: String,
    #[serde(default, alias = "changes")]
    pub changelog: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
}

/// Parse and validate a manifest body. Every failure is a clear
/// error naming the problem — a malformed manifest never crashes
/// the check and never reports a phantom update.
pub fn parse_manifest(body: &[u8]) -> Result<UpdateManifest> {
    if body.len() > MAX_MANIFEST_BYTES {
        return Err(EngineError::InvalidState(
            "The update server returned a manifest that is too large to be a version manifest — check the update server address in Settings".to_string(),
        ));
    }
    let raw: UpdateManifest = serde_json::from_slice(body).map_err(|e| {
        EngineError::InvalidState(format!(
            "The update server returned a manifest Fetchline could not read (invalid JSON: {e}) — check the update server address in Settings"
        ))
    })?;
    let version = raw.version.trim().to_string();
    if version.is_empty() {
        return Err(EngineError::InvalidState(
            "The update manifest has no version — check the update server address in Settings".to_string(),
        ));
    }
    if parse_version(&version).is_none() {
        return Err(EngineError::InvalidState(format!(
            "The update manifest has a version Fetchline cannot read: '{version}' — check the update server address in Settings"
        )));
    }
    if let Some(url) = raw.download_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        if !matches!(url.split_once("://").map(|(s, _)| s), Some("http") | Some("https")) {
            return Err(EngineError::InvalidState(format!(
                "The update manifest has a download address Fetchline cannot use: '{url}'"
            )));
        }
    }
    let mut changelog: Vec<String> = raw
        .changelog
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if let Some(notes) = raw.notes.as_deref() {
        changelog.extend(
            notes
                .lines()
                .map(|l| l.trim().trim_start_matches(['-', '*', ' ']).trim().to_string())
                .filter(|l| !l.is_empty()),
        );
    }
    changelog.truncate(100);
    Ok(UpdateManifest {
        version,
        changelog,
        notes: None,
        download_url: raw.download_url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
    })
}

/// The result of one update check, as the UI shows it. An
/// update is NEVER installed by the check: `update_available`
/// only means the dialog should ask the user, and
/// `updater_configured` says whether an in-app install exists
/// at all (it does not, in this build — see the module docs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub changelog: Vec<String>,
    pub download_url: Option<String>,
    /// When the check ran, UTC ISO-8601 — also stored in settings
    /// as `last_update_check_at`, which is what the weekly timer
    /// reads to decide whether a check is due.
    pub checked_at: String,
    pub source_url: String,
    pub updater_configured: bool,
}

/// Build the result from a parsed manifest. `now_unix` is the
/// time the check ran, so the stored last-check time and the
/// result agree.
pub fn result_from_manifest(
    manifest: &UpdateManifest,
    source_url: &str,
    now_unix: i64,
) -> UpdateCheckResult {
    UpdateCheckResult {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: manifest.version.clone(),
        update_available: is_newer(&manifest.version, CURRENT_VERSION),
        changelog: manifest.changelog.clone(),
        download_url: manifest.download_url.clone(),
        checked_at: unix_to_iso(now_unix),
        source_url: source_url.to_string(),
        updater_configured: UPDATER_CONFIGURED,
    }
}

/// Fetch the manifest from `url` with the engine's HTTP client
/// (so the configured proxy and timeout apply). A non-2xx answer
/// is a clear error naming the status; a network failure is a
/// clear error naming the failure — never a silent "up to date".
pub async fn fetch_manifest(client: &reqwest::Client, url: &str) -> Result<UpdateManifest> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| {
        EngineError::InvalidUrl("The update server address is not a valid URL".to_string())
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(EngineError::InvalidUrl(
            "The update server address must start with http:// or https://".to_string(),
        ));
    }
    let response = client
        .get(url.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| {
            EngineError::InvalidState(format!(
                "Could not reach the update server ({url}) — check your connection and try again: {e}"
            ))
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(EngineError::InvalidState(format!(
            "The update server answered with status {status} — check the update server address in Settings"
        )));
    }
    let body = response.bytes().await.map_err(|e| {
        EngineError::InvalidState(format!("Could not read the update server's answer: {e}"))
    })?;
    parse_manifest(&body)
}

/// Is an automatic check due at `now` (Unix seconds)? Due when
/// the weekly setting is on and there has never been a check, or
/// the last one is at least a week old. A malformed stored time
/// is treated as "never checked" rather than crashing the tick.
pub fn weekly_check_due(check_updates_weekly: bool, last_check_at: Option<&str>, now: i64) -> bool {
    if !check_updates_weekly {
        return false;
    }
    match last_check_at.and_then(crate::timeutil::parse_iso_to_unix) {
        None => true,
        Some(last) => now >= last + UPDATE_CHECK_INTERVAL_SECS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_is_numeric_not_lexical() {
        assert_eq!(compare_versions("1.10.0", "1.9.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("1.2.3", "1.2.2"), Some(Ordering::Greater));
        assert_eq!(compare_versions("1.2.2", "1.2.3"), Some(Ordering::Less));
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Some(Ordering::Equal));
        assert_eq!(compare_versions("2.0.0", "1.99.99"), Some(Ordering::Greater));
        // Shorter is equal when the missing parts are zero.
        assert_eq!(compare_versions("1.2", "1.2.0"), Some(Ordering::Equal));
        assert_eq!(compare_versions("1.2", "1.2.1"), Some(Ordering::Less));
        // A leading `v` and whitespace are tolerated.
        assert_eq!(compare_versions("v1.10.0", "1.9.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions(" 1.2.3 ", "1.2.2"), Some(Ordering::Greater));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(!is_newer("1.9.9", "1.10.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
    }

    #[test]
    fn prerelease_is_older_than_the_release() {
        assert_eq!(
            compare_versions("1.0.0-beta", "1.0.0"),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_versions("1.0.0-beta.1", "1.0.0-beta.2"),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_versions("1.0.0-beta.2", "1.0.0-beta.10"),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_versions("1.0.0-alpha", "1.0.0-beta"),
            Some(Ordering::Less)
        );
        assert_eq!(compare_versions("1.0.0-rc.1", "1.0.0"), Some(Ordering::Less));
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
    }

    #[test]
    fn malformed_versions_are_rejected() {
        for bad in ["", "v", "1.2.x", "1..2", "1.2.3-", "1.2.3-beta..1", "one.two.three"] {
            assert!(parse_version(bad).is_none(), "{bad}");
        }
        // Long dotted versions parse fine (no fixed part limit).
        assert!(parse_version("1.2.3.4.5.6.7.8.9.10.11").is_some());
        assert_eq!(compare_versions("1.2.x", "1.2.3"), None);
        assert!(!is_newer("banana", "0.1.0"));
    }

    #[test]
    fn manifest_parsing_accepts_changelog_notes_and_changes() {
        let m = parse_manifest(
            br#"{"version": "0.2.0", "changelog": ["Faster downloads"], "download_url": "https://updates.fetchline.app/releases/fetchline-0.2.0.msi"}"#,
        )
        .unwrap();
        assert_eq!(m.version, "0.2.0");
        assert_eq!(m.changelog, vec!["Faster downloads"]);
        assert!(m.download_url.is_some());

        let m = parse_manifest(br#"{"version": "0.3.0", "notes": "- First thing\n- Second thing\n"}"#).unwrap();
        assert_eq!(m.changelog, vec!["First thing", "Second thing"]);

        let m = parse_manifest(br#"{"version": "0.3.0", "changes": ["Via changes"]}"#).unwrap();
        assert_eq!(m.changelog, vec!["Via changes"]);
    }

    #[test]
    fn manifest_parsing_rejects_bad_manifests() {
        assert!(parse_manifest(b"{not json").is_err());
        assert!(parse_manifest(br#"{"changelog": []}"#).is_err());
        assert!(parse_manifest(br#"{"version": ""}"#).is_err());
        assert!(parse_manifest(br#"{"version": "banana"}"#).is_err());
        assert!(parse_manifest(br#"{"version": "0.2.0", "download_url": "ftp://example.com/x.msi"}"#).is_err());
        assert!(parse_manifest(&vec![b'x'; MAX_MANIFEST_BYTES + 1]).is_err());
    }

    #[test]
    fn weekly_due_logic() {
        assert!(weekly_check_due(true, None, 1_000_000));
        assert!(!weekly_check_due(false, None, 1_000_000));
        // Checked 6 days 20 hours ago: not due; exactly 7 days ago: due.
        assert!(!weekly_check_due(true, Some("2026-11-08T00:00:00Z"), 1_794_688_200));
        assert!(weekly_check_due(true, Some("2026-11-07T20:30:00Z"), 1_794_688_200));
        // A malformed stored time is "never checked", not a crash.
        assert!(weekly_check_due(true, Some("not a time"), 1_794_688_200));
    }
}
