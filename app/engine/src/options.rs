//! Options dialog, finish (S09): Site logins, Dial-Up, Sounds.
//!
//! Three tabs that were deferred in the core Settings milestone:
//! - Site logins: per-site saved credentials, used by the engine's
//!   probe/download as HTTP Basic auth for that host. Passwords go
//!   through the existing `secret.rs` path (DPAPI on Windows,
//!   base64-obfuscated on Linux — the same standing limitation as
//!   proxy passwords, see `secret.rs` / build-log.md). The password
//!   is never returned to the UI: `SiteLoginRow` only carries
//!   `has_password`, like `ProxyConfigRow`.
//! - Dial-Up: IDM-era dial-up settings. These are legacy on modern
//!   Windows: the tab stores the settings, and only the part that
//!   still has a real engine behaviour is wired in — "hang up after
//!   completion" maps onto the existing on-completion action path
//!   (a `hangup` pending action, executed on Windows with
//!   `rasdial <name> /disconnect`, refused off Windows like
//!   shutdown/hibernate). Auto-dial and redial attempts are stored
//!   and validated, and are honoured only where a dial-up
//!   connection exists (Windows RAS) — this VM has no RAS, so no
//!   dial or redial is ever executed here, and none is faked.
//!   Redial attempts are dial-up connection attempts, NOT download
//!   retries: they never change `settings.max_retries`.
//! - Sounds: per-event sounds (download complete, download
//!   failed, queue finished). A sound is a WAV file, enabled or
//!   disabled per event. The engine emits the event (recorded in
//!   the manager's sound log, with the resolved file) and plays
//!   it on Windows only (PowerShell `SoundPlayer`); on this Linux
//!   VM playback is never heard — tests assert the event fires
//!   and the configured file resolves, not that audio played.

use serde::{Deserialize, Serialize};

use crate::error::{EngineError, Result};

/// The three sound events, in UI order. Anything else is rejected
/// at save time — there is no fourth event to configure.
pub const SOUND_EVENTS: [&str; 3] = ["download_complete", "download_failed", "queue_finished"];

/// Human label for a sound event, for the UI. Fresh wording.
pub fn sound_event_label(event: &str) -> &'static str {
    match event {
        "download_complete" => "Download complete",
        "download_failed" => "Download failed",
        "queue_finished" => "Queue finished",
        _ => "Unknown event",
    }
}

pub fn is_known_sound_event(event: &str) -> bool {
    SOUND_EVENTS.contains(&event)
}

/// One site login, as the UI sees it. No password — only whether
/// one is stored, so the dialog can show "password saved" without
/// ever receiving the secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteLoginRow {
    pub id: String,
    pub host: String,
    pub username: String,
    /// "basic" only: the engine only sends HTTP Basic auth.
    pub auth_type: String,
    pub has_password: bool,
}

/// What the UI sends when saving a site login. `password: None`
/// keeps the stored password (an edit that does not touch it);
/// `Some(p)` sets it. An empty password is never accepted — a
/// login without a password cannot authenticate, and the schema
/// column is NOT NULL, so there is no "clear password" state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteLoginInput {
    pub id: Option<String>,
    pub host: String,
    pub username: String,
    pub password: Option<String>,
    pub auth_type: String,
}

/// One event's sound setting, as the UI sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundSetting {
    pub event: String,
    pub enabled: bool,
    pub file: Option<String>,
}

/// What the UI sends when saving one event's sound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundSettingInput {
    pub event: String,
    pub enabled: bool,
    /// `None` (or an empty string) means "no sound file picked".
    pub file: Option<String>,
}

/// One sound event the engine emitted, with the file it resolved
/// to (if any). The log is the testable surface on this VM: audio
/// playback itself never happens here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundEventRecord {
    pub event: String,
    pub file: Option<String>,
    pub at_unix: i64,
}

/// Clean a host the user typed: strip a scheme, strip any path /
/// query / fragment, strip a port (matching is by host, and
/// `Url::host_str` never carries a port), lowercase, drop a
/// trailing dot. "https://Example.COM:8080/some/path" becomes
/// "example.com". Returns `None` when nothing usable is left.
pub fn clean_site_host(raw: &str) -> Option<String> {
    let mut h = raw.trim().to_ascii_lowercase();
    if let Some(rest) = h.strip_prefix("https://") {
        h = rest.to_string();
    } else if let Some(rest) = h.strip_prefix("http://") {
        h = rest.to_string();
    }
    if let Some(cut) = h.find(|c| c == '/' || c == '?' || c == '#') {
        h.truncate(cut);
    }
    // A userinfo prefix ("user@host") is not a host: refuse it by
    // leaving the '@' in, so the validation below rejects it.
    if let Some(colon) = h.rfind(':') {
        let after = &h[colon + 1..];
        if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
            h.truncate(colon);
        }
    }
    let h = h.trim_end_matches('.').to_string();
    if h.is_empty() {
        None
    } else {
        Some(h)
    }
}

/// Validate a site login before anything is stored. `has_password`
/// is whether a password is already stored for this host (an edit
/// may keep it, a new login may not).
pub fn validate_site_login_input(input: &SiteLoginInput, has_password: bool) -> Result<String> {
    let host = clean_site_host(&input.host).ok_or_else(|| {
        EngineError::InvalidState("Give the site a host, for example: example.com".to_string())
    })?;
    if host.contains(char::is_whitespace) || host.contains('@') {
        return Err(EngineError::InvalidState(
            "The site host is just a name or address, without spaces or a username".to_string(),
        ));
    }
    if input.username.trim().is_empty() {
        return Err(EngineError::InvalidState(
            "Give the login a username".to_string(),
        ));
    }
    match &input.password {
        Some(p) if p.is_empty() => {
            return Err(EngineError::InvalidState(
                "Give the login a password — a site login without one cannot sign in".to_string(),
            ))
        }
        None if !has_password => {
            return Err(EngineError::InvalidState(
                "Give the login a password".to_string(),
            ))
        }
        _ => {}
    }
    if input.auth_type != "basic" {
        return Err(EngineError::InvalidState(format!(
            "Only Basic auth is applied to site logins — “{}” is stored nowhere",
            input.auth_type
        )));
    }
    Ok(host)
}

/// Validate one event's sound setting. A file, when set, must be a
/// `.wav` file — the player path only knows WAV.
pub fn validate_sound_setting(input: &SoundSettingInput) -> Result<Option<String>> {
    if !is_known_sound_event(&input.event) {
        return Err(EngineError::InvalidState(format!(
            "Unknown sound event: {}",
            input.event
        )));
    }
    match input
        .file
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
    {
        None => Ok(None),
        Some(f) => {
            if !f.to_ascii_lowercase().ends_with(".wav") {
                return Err(EngineError::InvalidState(
                    "Sounds must be WAV files — pick a file ending in .wav".to_string(),
                ));
            }
            Ok(Some(f.to_string()))
        }
    }
}

/// Validate the dial-up fields of a settings save. The connection
/// name may be empty (no dial-up connection configured); redial
/// attempts are 0–10, where 0 means "do not redial".
pub fn validate_dialup(redial_attempts: i64) -> Result<()> {
    if !(0..=10).contains(&redial_attempts) {
        return Err(EngineError::InvalidState(
            "Redial attempts must be between 0 and 10".to_string(),
        ));
    }
    Ok(())
}

/// The effective completion action for a queue that drained
/// cleanly, given the queue's own action and the dial-up
/// "hang up after completion" flag. The queue's own action wins;
/// hang-up applies only when the queue has no other action, so a
/// queue never arms two actions at once (one pending action per
/// queue id). Returns `None` when nothing is armed.
pub fn effective_completion_action<'a>(
    queue_action: &'a str,
    hangup_after_completion: bool,
) -> Option<&'a str> {
    if queue_action != "none" {
        Some(queue_action)
    } else if hangup_after_completion {
        Some("hangup")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_site_host_strips_scheme_port_and_path() {
        assert_eq!(
            clean_site_host("https://Example.COM:8080/some/path"),
            Some("example.com".to_string())
        );
        assert_eq!(
            clean_site_host("  files.example.org. "),
            Some("files.example.org".to_string())
        );
        assert_eq!(clean_site_host(""), None);
        assert_eq!(clean_site_host("https://"), None);
    }

    #[test]
    fn sound_validation_rejects_non_wav_and_unknown_event() {
        assert!(validate_sound_setting(&SoundSettingInput {
            event: "download_complete".to_string(),
            enabled: true,
            file: Some("ding.mp3".to_string()),
        })
        .is_err());
        assert!(validate_sound_setting(&SoundSettingInput {
            event: "no_such_event".to_string(),
            enabled: true,
            file: None,
        })
        .is_err());
        assert_eq!(
            validate_sound_setting(&SoundSettingInput {
                event: "download_complete".to_string(),
                enabled: true,
                file: Some("  ".to_string()),
            })
            .unwrap(),
            None
        );
    }

    #[test]
    fn effective_action_prefers_the_queue_action_over_hangup() {
        assert_eq!(
            effective_completion_action("shutdown", true),
            Some("shutdown")
        );
        assert_eq!(effective_completion_action("none", true), Some("hangup"));
        assert_eq!(effective_completion_action("none", false), None);
    }

    #[test]
    fn dialup_redial_range_is_0_to_10() {
        assert!(validate_dialup(0).is_ok());
        assert!(validate_dialup(10).is_ok());
        assert!(validate_dialup(11).is_err());
        assert!(validate_dialup(-1).is_err());
    }
}
