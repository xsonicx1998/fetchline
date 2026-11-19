//! Global speed limiter (S12): one token bucket shared by every
//! running download, so the cap is a cap on the app's total
//! bandwidth, not per download. A rate of 0 means unlimited.
//!
//! The bucket holds up to one second's worth of tokens (the burst),
//! refills continuously, and a segment task waits — without holding
//! any lock — until enough tokens exist for the chunk it just read.
//! Waiting happens between chunks, so pausing and checkpointing are
//! never blocked by the limiter.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

struct Bucket {
    /// Tokens currently available (fractional, up to `rate`).
    tokens: f64,
    last_refill: Instant,
}

/// A token bucket. Cheap to clone (one `Arc` in the manager) and safe
/// to share across every segment task.
pub struct RateLimiter {
    /// Bytes per second; 0 = unlimited.
    rate_bps: AtomicU64,
    bucket: StdMutex<Bucket>,
}

impl RateLimiter {
    pub fn new(rate_bps: u64) -> Self {
        Self {
            rate_bps: AtomicU64::new(rate_bps),
            bucket: StdMutex::new(Bucket {
                tokens: rate_bps as f64,
                last_refill: Instant::now(),
            }),
        }
    }

    /// Current cap in bytes per second (0 = unlimited).
    pub fn rate_bps(&self) -> u64 {
        self.rate_bps.load(Ordering::Relaxed)
    }

    /// Change the cap live. A download already running picks the new
    /// rate up on its next chunk — no restart is needed. Shrinking
    /// the cap also shrinks the stored tokens so the old burst cannot
    /// be spent at the new, lower rate.
    pub fn set_rate_bps(&self, rate_bps: u64) {
        self.rate_bps.store(rate_bps, Ordering::Relaxed);
        let mut bucket = self.bucket.lock().unwrap();
        bucket.tokens = bucket.tokens.min(rate_bps as f64);
        bucket.last_refill = Instant::now();
    }

    /// Wait until `bytes` may be spent. Returns at once when the
    /// limiter is unlimited, or when enough tokens are already banked.
    ///
    /// A single chunk can be larger than one second of rate (a fast
    /// server hands over 64 KiB at once while the cap is 16 KiB/s),
    /// and the bucket never holds more than one second of tokens —
    /// so a large acquire is spent in rate-sized pieces instead of
    /// waiting forever for a balance it can never reach.
    pub async fn acquire(&self, bytes: u64) {
        let rate = self.rate_bps();
        if rate == 0 || bytes == 0 {
            return;
        }
        let mut remaining = bytes;
        while remaining > 0 {
            let piece = remaining.min(rate);
            self.acquire_piece(piece, rate).await;
            remaining -= piece;
        }
    }

    /// Wait until `bytes` (at most one second of rate) may be spent.
    async fn acquire_piece(&self, bytes: u64, rate: u64) {
        loop {
            let wait = {
                let mut bucket = self.bucket.lock().unwrap();
                let now = Instant::now();
                let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
                bucket.tokens = (bucket.tokens + elapsed * rate as f64).min(rate as f64);
                bucket.last_refill = now;
                if bucket.tokens >= bytes as f64 {
                    bucket.tokens -= bytes as f64;
                    return;
                }
                Duration::from_secs_f64((bytes as f64 - bucket.tokens) / rate as f64)
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_returns_at_once() {
        let limiter = RateLimiter::new(0);
        let start = Instant::now();
        limiter.acquire(10 * 1024 * 1024).await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn capped_acquire_waits_for_tokens() {
        // 10 KiB/s: the first 10 KiB are free (initial burst), the
        // next 10 KiB must wait about a second.
        let limiter = RateLimiter::new(10 * 1024);
        limiter.acquire(10 * 1024).await;
        let start = Instant::now();
        limiter.acquire(10 * 1024).await;
        let waited = start.elapsed();
        assert!(waited >= Duration::from_millis(800), "waited {waited:?}");
        assert!(waited < Duration::from_secs(3), "waited {waited:?}");
    }

    #[tokio::test]
    async fn acquire_larger_than_rate_completes() {
        // A 64 KiB chunk at a 16 KiB/s cap: the bucket never holds
        // more than one second of tokens, so a naive single wait
        // would never reach 64 KiB. The chunk must be spent in
        // pieces instead (~3 s of waiting, not forever).
        let limiter = RateLimiter::new(16 * 1024);
        let start = Instant::now();
        limiter.acquire(64 * 1024).await;
        let waited = start.elapsed();
        assert!(waited >= Duration::from_secs(2), "waited {waited:?}");
        assert!(waited < Duration::from_secs(6), "waited {waited:?}");
    }

    #[tokio::test]
    async fn rate_change_takes_effect_live() {
        let limiter = RateLimiter::new(1024);
        limiter.acquire(1024).await;
        limiter.set_rate_bps(0);
        assert_eq!(limiter.rate_bps(), 0);
        let start = Instant::now();
        limiter.acquire(1024 * 1024).await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
