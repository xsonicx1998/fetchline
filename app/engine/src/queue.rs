//! Queue completion actions.
//!
//! When a queue drains, the manager registers a `PendingAction` and the
//! UI decides what to do with it. Shutdown / hibernate are never
//! executed silently: the UI shows a countdown with a Cancel button and
//! only calls `execute_queue_action` after the user confirms (or the
//! countdown runs out). Open-file and run-program actions run at once.

use serde::{Deserialize, Serialize};

use crate::error::{EngineError, Result};

/// A completion action waiting for the user (or the countdown) to
/// confirm it. Kept in memory only — if the app closes before it is
/// confirmed, nothing runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAction {
    pub queue_id: String,
    pub queue_name: String,
    /// one of: none | open_file | run_program | shutdown | hibernate
    pub action: String,
    pub program: Option<String>,
    /// For open_file: the file that just finished.
    pub file_path: Option<String>,
    pub created_at_unix: i64,
}

/// Validate a completion action the way `update_queue` always
/// has: the action must be a known one, and `run_program` needs a
/// program to run. The site grabber (S06, Milestone 4) stores the
/// same actions on its projects, so both validate through this one
/// function — one vocabulary, one set of messages, no drift.
pub fn validate_completion_action(action: &str, program: Option<&str>) -> Result<()> {
    if !matches!(
        action,
        "none" | "open_file" | "run_program" | "shutdown" | "hibernate"
    ) {
        return Err(EngineError::InvalidState(format!(
            "Unknown completion action: {action}"
        )));
    }
    if action == "run_program" && program.map(|p| p.trim().is_empty()).unwrap_or(true) {
        return Err(EngineError::InvalidState(
            "Choose a program to run when the queue finishes".to_string(),
        ));
    }
    Ok(())
}

/// Carry out a confirmed completion action. `none` is a no-op.
/// Shutdown / hibernate only exist on Windows — on any other OS this
/// is an error rather than a silent no-op, so a bug can never silently
/// skip the action the user asked for. `hangup` is the Dial-Up
/// tab's "hang up after completion": on Windows it disconnects
/// the named dial-up connection with `rasdial <name> /disconnect`
/// (the connection name travels in `PendingAction.program`, the
/// same slot `run_program` uses — the two actions never coexist
/// on one queue, so the slot is unambiguous). RAS execution has
/// never run on this VM: there is no dial-up connection here to
/// hang up, and the Windows branch is unproven like the
/// shutdown/hibernate branches. Off Windows it is the same
/// honest error, not a silent skip.
pub fn execute_action(action: &PendingAction) -> Result<()> {
    match action.action.as_str() {
        "none" => Ok(()),
        "hangup" => {
            let name = action.program.as_ref().ok_or_else(|| {
                EngineError::InvalidState(
                    "No dial-up connection name is set — pick one in Settings, Dial-Up".to_string(),
                )
            })?;
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                std::process::Command::new("rasdial")
                    .args([name.as_str(), "/disconnect"])
                    // Round 70 — no console flash for the hang-up.
                    .creation_flags(0x0800_0000)
                    .spawn()
                    .map_err(EngineError::from)?;
                Ok(())
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = name;
                Err(EngineError::InvalidState(
                    "Hanging up a dial-up connection is only available on Windows".to_string(),
                ))
            }
        }
        "open_file" => {
            let path = action.file_path.as_ref().ok_or_else(|| {
                EngineError::InvalidState("No finished file to open".to_string())
            })?;
            open_path(path)
        }
        "run_program" => {
            let program = action.program.as_ref().ok_or_else(|| {
                EngineError::InvalidState("No program set for this queue".to_string())
            })?;
            std::process::Command::new(program)
                .spawn()
                .map_err(EngineError::from)?;
            Ok(())
        }
        "shutdown" => {
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                std::process::Command::new("shutdown")
                    .args(["/s", "/t", "0"])
                    // Round 70 — no console flash for the shutdown.
                    .creation_flags(0x0800_0000)
                    .spawn()
                    .map_err(EngineError::from)?;
                Ok(())
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(EngineError::InvalidState(
                    "Shutting down the computer is only available on Windows".to_string(),
                ))
            }
        }
        "hibernate" => {
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                std::process::Command::new("shutdown")
                    .args(["/h"])
                    // Round 70 — no console flash for hibernate.
                    .creation_flags(0x0800_0000)
                    .spawn()
                    .map_err(EngineError::from)?;
                Ok(())
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(EngineError::InvalidState(
                    "Hibernating the computer is only available on Windows".to_string(),
                ))
            }
        }
        other => Err(EngineError::InvalidState(format!(
            "Unknown completion action: {other}"
        ))),
    }
}

/// Open a file or folder in the system default handler
/// (`cmd /C start` on Windows, `open` on macOS, `xdg-open` on
/// Linux). Shared by the queue completion action (`open_file`) and
/// by "Open folder" in the progress dialog (S03) — one opener.
/// Must be called with a path that has already been validated.
pub(crate) fn open_path(path: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd")
            .args(["/C", "start", "", path])
            // Round 70 — no console flash for Open file/folder.
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(EngineError::from)?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(EngineError::from)?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(EngineError::from)?;
    }
    Ok(())
}
