//! Fetchline download engine.
//!
//! Clean-room implementation of download-manager behaviour, written from
//! the recon map (`replica/recon.md`) and the architecture
//! (`replica/architecture.md`). No code, assets or copy from the original
//! application is used.
//!
//! Layout:
//! - `probe`   — HEAD probe for size / range support / ETag / filename.
//! - `store`   — SQLite persistence (schema from `replica/schema.sql`).
//! - `manager` — segmented download, pause/resume, retry, progress events.
//! - `limiter` — global token-bucket speed limit (S12).
//! - `proxy`   — proxy configuration + client building (S09).
//! - `secret`  — password protection (DPAPI on Windows).

pub mod antivirus;
pub mod clipboard;
pub mod error;
pub mod ftp;
pub mod grabber;
pub mod kerberos;
pub mod limiter;
pub mod links;
pub mod manager;
pub mod native_host;
pub mod ntlm;
pub mod options;
pub mod probe;
pub mod proxy;
pub mod queue;
pub mod quota;
pub mod secret;
pub mod shell;
pub mod store;
pub mod timeutil;
pub mod update;
pub mod video;
pub mod ytdlp;

pub use antivirus::{
    run_scan, test_scanner, validate_scan_timeout, ScanResult, DEFAULT_SCAN_TIMEOUT_SECS,
    MAX_SCAN_TIMEOUT_SECS, MIN_SCAN_TIMEOUT_SECS, SCAN_CLEAN, SCAN_FAILED, SCAN_INFECTED,
    SCAN_NOT_CONFIGURED, SCAN_SCANNING, SCAN_STATUSES, SCAN_TIMED_OUT,
};
pub use clipboard::ClipboardOffer;
pub use error::EngineError;
pub use grabber::{GrabberFilters, GrabberProjectRow, MatchedFile};
pub use kerberos::{
    build_neg_token_init, build_neg_token_resp, kerberos_head, kerberos_request,
    kerberos_request_with_initiator, negotiate_response_token, new_kerberos_initiator,
    parse_spnego_token, SpnegoInitiator, SpnegoStep, SpnegoToken,
};
pub use limiter::RateLimiter;
pub use links::{classify_kind, extract_links, filename_from_link_url, LinkKind, PageLink};
pub use manager::{
    BatchResult, DownloadManager, DrainSummary, InvalidLine, ProgressEvent, RefreshAddressResult,
    RetryInfo, SegmentProgress, SyncFileResult, SyncResult,
};
pub use native_host::{HostMessage, HostReply};
pub use ntlm::{ntlm_head, ntlm_request, NtlmBodyReader};
pub use options::{
    SiteLoginInput, SiteLoginRow, SoundEventRecord, SoundSetting, SoundSettingInput, SOUND_EVENTS,
};
pub use probe::probe_with_auth;
pub use probe::{probe, ProbeInfo};
pub use proxy::{
    basic_proxy_auth_header, offered_proxy_auth_schemes, proxy_auth_not_offered_error,
    proxy_auth_order, resolve_proxy_auth, ProxyAuthScheme, ProxyConfig, ProxyConfigInput,
    ProxyConfigRow, PROXY_AUTH_BASIC, PROXY_AUTH_KERBEROS, PROXY_AUTH_NEGOTIATE_AUTO,
    PROXY_AUTH_NTLM,
};
pub use queue::PendingAction;
pub use quota::{QuotaGate, QuotaStatus, QUOTA_MB_BYTES};
pub use store::{
    CategoryRow, DownloadRow, HistoryRow, PendingLinkRow, QueueRow, SegmentRow, Settings, Store,
};
pub use update::{
    compare_versions, fetch_manifest, is_newer, parse_manifest, parse_version,
    result_from_manifest, weekly_check_due, UpdateCheckResult, UpdateManifest, CURRENT_VERSION,
    DEFAULT_UPDATE_URL, MAX_MANIFEST_BYTES, UPDATER_CONFIGURED, UPDATE_CHECK_INTERVAL_SECS,
};
pub use video::{DetectedVideo, VideoVariantInput};
