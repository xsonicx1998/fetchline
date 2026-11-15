//! Download quota + per-download speed cap (S12).
//!
//! # Quota semantics (chosen, documented — see `replica/features.csv`)
//!
//! * **Window anchor: first use.** The window starts when the first
//!   byte is counted after the quota is enabled, after a rollover,
//!   or after a manual reset — not at a fixed epoch boundary. A
//!   fixed boundary would make a quota set at 23:00 expire an hour
//!   later; first-use anchoring gives the user the full Y hours
//!   they configured, whenever they start downloading.
//! * **What counts: actual downloaded bytes only.** Every byte a
//!   segment writes to a `.part` file counts, from every path that
//!   downloads (manual, queue, drain, grabber, sync re-download).
//!   Probe/HEAD bytes never count (a probe moves no file bytes),
//!   and a byte re-downloaded after a resume counts again — it was
//!   downloaded again. Bytes are recorded per chunk, as they are
//!   written, so a download paused mid-file has already banked
//!   what it downloaded.
//! * **When reached: the queue pauses.** The running download is
//!   paused (marked `quota_paused`, so only it is auto-resumed)
//!   and no new download starts until the window rolls over. The
//!   queue itself is not failed: its files stay queued.
//! * **Rollover: automatic.** When the window end passes, the
//!   usage resets and quota-paused downloads resume — checked on
//!   the 30 s scheduler tick and before any start, so a rollover
//!   is picked up within a tick (or at once on the next start).
//! * **Manual action: reset.** `reset_quota` starts a fresh window
//!   at once (usage 0, window anchored now) and resumes
//!   quota-paused downloads. There is no separate "resume now"
//!   that keeps the old window: resuming into an exhausted window
//!   would pause the download again on its next chunk.
//! * **Clock seam (tests):** every quota method has an `_at(now)`
//!   variant taking Unix seconds, mirroring `scheduler_tick_at`.
//!   Tests drive the window with fake `now` values instead of
//!   waiting hours — a 1-hour window rolls over by calling the
//!   `_at` method with `now + 3601`.
//!
//! # Per-download cap
//!
//! A KB/s cap on one download (`0` / `NULL` = no per-download cap,
//! the default). It is enforced by a second token bucket, one per
//! running download, alongside the existing global bucket: every
//! chunk waits for tokens from the global bucket AND from the
//! per-download bucket, so the effective rate is the min of the
//! two. The cap is live-changeable on a running download, like the
//! global cap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::store::Store;

/// One megabyte, in bytes. Quota limits are configured in MB.
pub const QUOTA_MB_BYTES: u64 = 1024 * 1024;

/// Quota validation bounds (rejected, not clamped — a typo must be
/// visible, the BUG-008 rule): 1 MB .. 1 TB, 1 hour .. 30 days.
pub const MIN_QUOTA_MB: i64 = 1;
pub const MAX_QUOTA_MB: i64 = 1_000_000;
pub const MIN_QUOTA_HOURS: i64 = 1;
pub const MAX_QUOTA_HOURS: i64 = 720;

/// End of a window anchored at `window_start_unix` lasting
/// `window_hours` hours. `None` when no window is open.
pub fn window_end_unix(window_start_unix: Option<i64>, window_hours: i64) -> Option<i64> {
    window_start_unix.map(|start| start + window_hours.saturating_mul(3600))
}

/// Has the window anchored at `window_start_unix` ended by `now`?
/// A window that never opened has nothing to roll over.
pub fn window_expired(window_start_unix: Option<i64>, window_hours: i64, now: i64) -> bool {
    match window_end_unix(window_start_unix, window_hours) {
        Some(end) => now >= end,
        None => false,
    }
}

/// The quota's live state, as the UI shows it (S12): the configured
/// limit, how much of the current window is used, when the window
/// ends, whether the quota is reached, and how many downloads are
/// paused waiting for the window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaStatus {
    pub enabled: bool,
    pub limit_mb: Option<i64>,
    pub window_hours: Option<i64>,
    pub used_bytes: i64,
    pub window_start_unix: Option<i64>,
    pub window_end_unix: Option<i64>,
    pub reached: bool,
    pub quota_paused_downloads: i64,
}

/// The per-chunk quota gate one running download carries. Created
/// per run from the settings in force when the run starts — like
/// `max_connections`, a quota enabled mid-download applies to the
/// next download, not to the running one (its bytes were never
/// counted, so counting them from mid-run would under-count the
/// window). `None` when no quota is enabled: segment code then
/// skips the gate entirely, exactly as before.
pub struct QuotaGate {
    store: Arc<Store>,
    limit_bytes: u64,
    window_secs: i64,
    /// Set when this gate's download is paused by the quota, so
    /// the run loop marks the row `quota_paused` (and only then).
    /// The pause flag itself is the run's own flag, shared with
    /// the gate so a reached quota stops the segment tasks at the
    /// next chunk boundary through the existing pause path.
    pause: Arc<AtomicBool>,
    hit: AtomicBool,
}

impl QuotaGate {
    pub fn new(
        store: Arc<Store>,
        limit_bytes: u64,
        window_secs: i64,
        pause: Arc<AtomicBool>,
    ) -> Self {
        Self {
            store,
            limit_bytes,
            window_secs,
            pause,
            hit: AtomicBool::new(false),
        }
    }

    /// Was this download paused by the quota (as opposed to the user)?
    pub fn hit(&self) -> bool {
        self.hit.load(Ordering::SeqCst)
    }

    /// Record `bytes` just written, against the persisted window.
    /// Anchors a new window at `now` when none is open or the old
    /// one expired, then adds the bytes. Returns true when the
    /// quota is now reached — and, when it is, sets the run's pause
    /// flag so the segment tasks stop at the next chunk boundary.
    /// A storage failure never fails the download: the bytes are
    /// lost from the window (and the run continues) rather than
    /// erroring a download that already wrote them.
    pub fn record(&self, bytes: u64, now: i64) -> bool {
        let reached = match self.store.record_quota_bytes(bytes, self.window_secs, now) {
            Ok(used) => used >= self.limit_bytes,
            Err(_) => false,
        };
        if reached {
            self.hit.store(true, Ordering::SeqCst);
            self.pause.store(true, Ordering::SeqCst);
        }
        reached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_end_is_start_plus_hours() {
        assert_eq!(window_end_unix(Some(1_000_000), 5), Some(1_000_000 + 18_000));
        assert_eq!(window_end_unix(None, 5), None);
    }

    #[test]
    fn window_expires_at_its_end_not_before() {
        assert!(!window_expired(Some(1_000_000), 1, 1_000_000 + 3599));
        assert!(window_expired(Some(1_000_000), 1, 1_000_000 + 3600));
        assert!(!window_expired(None, 1, 1_000_000 + 999_999));
    }
}
