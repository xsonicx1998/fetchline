//! Windows shell helpers for the "Open with..." dialog (Round 87.7).
//!
//! `OpenAs_RunDLL` parses its file argument out of rundll32's raw
//! command line — a path with spaces arrives quoted when it is
//! passed through `Command::args` (Rust quotes arguments that
//! contain spaces), and OpenAs_RunDLL does not strip the quotes,
//! so it looks for a literally-quoted file and shows nothing.
//! That is exactly the 87.5 failure, still present in the 87.6
//! fallback: the primary `ShellExecuteW` "openas" call failed
//! with error 31 on a good path, and the rundll32 fallback
//! silently did nothing. The command line built here must be
//! passed with `CommandExt::raw_arg` (verbatim, no quoting) so
//! the path arrives unquoted.

/// The raw rundll32 tail that opens the Windows "Open with"
/// dialog for `path`: `shell32.dll,OpenAs_RunDLL <path>`, the
/// path UNQUOTED on purpose (see the module docs). Pass the
/// whole string as one `raw_arg` to `rundll32.exe`.
pub fn openas_rundll32_cmdline(path: &str) -> String {
    format!("shell32.dll,OpenAs_RunDLL {path}")
}
