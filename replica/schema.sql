-- IDM rebuild — SQLite schema (single-user desktop app)
-- File: %APPDATA%/idm-rebuild/app.db  (WAL mode)
-- Secrets (site_logins.password_enc, proxy_configs.password_enc) are
-- encrypted with Windows DPAPI before storage. Never store plaintext.

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS categories (
  id TEXT PRIMARY KEY,               -- uuid
  name TEXT NOT NULL UNIQUE,
  default_folder TEXT NOT NULL,
  file_extensions TEXT NOT NULL DEFAULT '[]',  -- JSON array, e.g. ["mp4","mkv"]
  is_builtin INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS queues (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  type TEXT NOT NULL DEFAULT 'download'
    CHECK (type IN ('download', 'synchronisation')),
  is_main INTEGER NOT NULL DEFAULT 0,
  schedule_type TEXT NOT NULL DEFAULT 'one_time'
    CHECK (schedule_type IN ('one_time', 'periodic')),
  start_at TEXT,                     -- UTC ISO-8601, nullable = manual only
  stop_at TEXT,                      -- UTC ISO-8601, nullable
  start_on_startup INTEGER NOT NULL DEFAULT 0,
  -- Periodic synchronisation (S05/F04, 2026-11-15): a queue whose
  -- schedule_type is 'periodic' re-probes its files every
  -- sync_interval_minutes (minimum 5) and re-downloads only the
  -- files whose remote size/ETag/Last-Modified changed. The last
  -- sync's time and per-outcome counts are stored here so the UI
  -- can show "last sync" + "changed files" without a second table.
  sync_interval_minutes INTEGER,    -- NULL unless schedule_type = 'periodic'
  last_sync_at TEXT,                 -- UTC ISO-8601 of the last sync run
  last_sync_changed INTEGER NOT NULL DEFAULT 0,
  last_sync_unchanged INTEGER NOT NULL DEFAULT 0,
  last_sync_removed INTEGER NOT NULL DEFAULT 0,
  last_sync_failed INTEGER NOT NULL DEFAULT 0,
  last_sync_skipped INTEGER NOT NULL DEFAULT 0,
  on_complete_action TEXT NOT NULL DEFAULT 'none'
    CHECK (on_complete_action IN ('none','open_file','run_program','shutdown','hibernate')),
  on_complete_program TEXT,          -- path to run, when action = run_program
  download_limit_mb_per_hours TEXT,  -- JSON {"mb":200,"hours":5} or NULL
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS downloads (
  id TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  final_url TEXT,                    -- after redirects
  filename TEXT NOT NULL,
  save_path TEXT NOT NULL,           -- final path (without .part)
  category_id TEXT NOT NULL REFERENCES categories(id) ON DELETE RESTRICT,
  queue_id TEXT REFERENCES queues(id) ON DELETE SET NULL,
  grabber_project_id TEXT REFERENCES grabber_projects(id) ON DELETE SET NULL,
  queue_position INTEGER NOT NULL DEFAULT 0, -- order inside a queue (0 = unqueued)
  size_bytes INTEGER,                -- NULL = unknown (chunked / no Content-Length)
  downloaded_bytes INTEGER NOT NULL DEFAULT 0 CHECK (downloaded_bytes >= 0),
  status TEXT NOT NULL DEFAULT 'queued'
    CHECK (status IN ('queued','downloading','paused','complete','error','deleted')),
  speed_bps INTEGER NOT NULL DEFAULT 0,
  eta_seconds INTEGER,
  -- Per-download speed cap (S12, 2026-11-15): KB/s, NULL = no
  -- per-download cap (the global cap still applies). 0 is stored
  -- as NULL by the engine.
  speed_limit_kbps INTEGER,
  -- Set when the download was paused because the download quota
  -- was reached (S12): only these downloads are resumed
  -- automatically when the quota window rolls over or is reset —
  -- a download the user paused by hand is never auto-resumed.
  quota_paused INTEGER NOT NULL DEFAULT 0,
  -- Whether the probe found the server range-capable (S03 progress
  -- window, 2026-11-15): 1 = resumable, 0 = no ranges, NULL =
  -- unknown (a download added before this was stored).
  accept_ranges INTEGER,
  -- Antivirus scan result (S14, 2026-11-15): the outcome of the
  -- scanner hook run on completion — clean | infected |
  -- scan_failed | not_configured | timed_out | scanning — with
  -- the scanner's own detail and the time it finished. NULL =
  -- never scanned (a download completed before S14, or the scan
  -- has not been recorded yet). The scanner itself is external;
  -- Fetchline only records what it reported.
  scan_status TEXT,
  scan_detail TEXT,
  scan_scanned_at TEXT,
  referrer TEXT,
  headers_json TEXT NOT NULL DEFAULT '{}',
  cookies_json TEXT NOT NULL DEFAULT '{}',
  etag TEXT,
  last_modified TEXT,
  error_message TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  completed_at TEXT,
  CHECK (size_bytes IS NULL OR downloaded_bytes <= size_bytes),
  CHECK (status != 'complete' OR size_bytes IS NULL OR downloaded_bytes = size_bytes)
);

CREATE TABLE IF NOT EXISTS segments (
  id TEXT PRIMARY KEY,
  download_id TEXT NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
  segment_index INTEGER NOT NULL,
  start_byte INTEGER NOT NULL CHECK (start_byte >= 0),
  end_byte INTEGER,                  -- NULL = unknown size (single open segment)
  downloaded_bytes INTEGER NOT NULL DEFAULT 0 CHECK (downloaded_bytes >= 0),
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending','active','done','failed')),
  connection_id TEXT,
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE (download_id, segment_index),
  CHECK (end_byte IS NULL OR end_byte >= start_byte)
);

CREATE TABLE IF NOT EXISTS grabber_projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  start_url TEXT NOT NULL,
  filters_json TEXT NOT NULL DEFAULT '{}',  -- file types, depth, exclude patterns
  mode TEXT NOT NULL DEFAULT 'download'
    CHECK (mode IN ('explore','download')),
  schedule_json TEXT,                -- nullable, same shape as queues
  status TEXT NOT NULL DEFAULT 'idle'
    CHECK (status IN ('idle','exploring','downloading','done','failed','stopped')),
  -- Added in the site-grabber milestone (S06, 2026-11-14): live
  -- progress counts, the failure reason, and the matched files
  -- (JSON) so the progress view can list them and explore-only
  -- mode can show what was found without downloading.
  pages_explored INTEGER NOT NULL DEFAULT 0,
  files_matched INTEGER NOT NULL DEFAULT 0,
  files_downloaded INTEGER NOT NULL DEFAULT 0,
  files_failed INTEGER NOT NULL DEFAULT 0,
  error_message TEXT,
  matched_json TEXT NOT NULL DEFAULT '[]',
  -- Added in the site-grabber finish (S06, Milestone 4,
  -- 2026-11-15): the queue grabbed files go to (the main queue
  -- when the project names none) and the on-completion action,
  -- reusing the existing queue completion actions (none /
  -- open_file / run_program / shutdown / hibernate).
  queue_id TEXT NOT NULL DEFAULT 'queue-main',
  on_complete_action TEXT NOT NULL DEFAULT 'none'
    CHECK (on_complete_action IN ('none','open_file','run_program','shutdown','hibernate')),
  on_complete_program TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS site_logins (
  id TEXT PRIMARY KEY,
  host TEXT NOT NULL UNIQUE,
  username TEXT NOT NULL,
  password_enc TEXT NOT NULL,        -- DPAPI-encrypted
  auth_type TEXT NOT NULL DEFAULT 'basic'
    CHECK (auth_type IN ('basic','negotiate','ntlm','kerberos')),
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS proxy_configs (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  type TEXT NOT NULL DEFAULT 'http'
    CHECK (type IN ('http','ftp','socks')),
  host TEXT NOT NULL,
  port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
  username TEXT,
  password_enc TEXT,                 -- DPAPI-encrypted, nullable
  use_for TEXT NOT NULL DEFAULT '[]', -- JSON array of protocols, [] = all
  enabled INTEGER NOT NULL DEFAULT 0, -- only the enabled row is used
  bypass TEXT NOT NULL DEFAULT '[]', -- JSON array of hosts that skip the proxy
  auth TEXT NOT NULL DEFAULT 'negotiate-auto' -- basic | ntlm | kerberos | negotiate-auto
    CHECK (auth IN ('basic','ntlm','kerberos','negotiate-auto')),
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
  default_download_folder TEXT NOT NULL DEFAULT '',
  max_connections INTEGER NOT NULL DEFAULT 8 CHECK (max_connections BETWEEN 1 AND 32),
  speed_limit_kbps INTEGER,           -- NULL = unlimited
  language TEXT NOT NULL DEFAULT 'en',
  theme TEXT NOT NULL DEFAULT 'light' CHECK (theme IN ('light','dark')),
  browser_integration INTEGER NOT NULL DEFAULT 1,
  browser_start_drained INTEGER NOT NULL DEFAULT 1, -- Milestone 2 (S13 finish):
    -- 1 = a drained browser link starts downloading at once;
    -- 0 = it stays queued, the old behaviour
  -- Progress window (S03, 2026-11-15): 1 = open a separate
  -- progress window when a download starts (default); 0 = the
  -- table is the only progress view (double-click still opens it).
  show_progress_window INTEGER NOT NULL DEFAULT 1,
  antivirus_command TEXT,             -- NULL = no scanner configured
  -- Antivirus hook (S14, 2026-11-15): the on/off switch for the
  -- scan (default on — the command being empty still means no
  -- scan) and the scan timeout in seconds (5–600, default 60).
  -- A scan that runs longer is killed and recorded as timed out.
  antivirus_enabled INTEGER NOT NULL DEFAULT 1,
  antivirus_timeout_secs INTEGER NOT NULL DEFAULT 60
    CHECK (antivirus_timeout_secs BETWEEN 5 AND 600),
  file_types_auto TEXT NOT NULL DEFAULT '[]',
  excluded_sites TEXT NOT NULL DEFAULT '[]',
  check_updates_weekly INTEGER NOT NULL DEFAULT 1,
  -- Added in the settings milestone (S09/S12, 2026-11-14):
  start_on_startup INTEGER NOT NULL DEFAULT 0, -- stored flag only; registering
    -- the app in Windows autostart is out of scope, see build-log.md
  default_action TEXT NOT NULL DEFAULT 'start_now'
    CHECK (default_action IN ('start_now','save_for_later')),
  duplicate_action TEXT NOT NULL DEFAULT 'rename'
    CHECK (duplicate_action IN ('rename','overwrite')),
  confirm_delete INTEGER NOT NULL DEFAULT 1,
  connection_timeout_secs INTEGER NOT NULL DEFAULT 30
    CHECK (connection_timeout_secs BETWEEN 1 AND 300),
  max_retries INTEGER NOT NULL DEFAULT 5
    CHECK (max_retries BETWEEN 0 AND 10),
  -- Dial-Up tab (S09 finish, 2026-11-15): legacy settings, stored
  -- here. Only "hang up after completion" has an engine behaviour
  -- (it maps onto the on-completion action path); auto-dial and
  -- redial attempts are honoured only where a dial-up connection
  -- exists (Windows RAS, unproven on this VM) and are never faked.
  dialup_connection_name TEXT,        -- NULL = no dial-up connection set
  dialup_auto_dial INTEGER NOT NULL DEFAULT 0,
  dialup_hangup_after_completion INTEGER NOT NULL DEFAULT 0,
  dialup_redial_attempts INTEGER NOT NULL DEFAULT 0
    CHECK (dialup_redial_attempts BETWEEN 0 AND 10),
  -- Update check (2026-11-15): Fetchline's own release server
  -- (configurable; the default is a placeholder that must be
  -- replaced with the real release server before shipping — see
  -- app/engine/src/update.rs) and the last-check time the weekly
  -- timer reads. Never IDM's servers or a third-party feed.
  update_url TEXT NOT NULL DEFAULT 'https://updates.fetchline.app/releases/latest.json',
  last_update_check_at TEXT,       -- NULL = never checked
  -- Download quota (S12, 2026-11-15): X MB every Y hours.
  -- `quota_enabled` is the on/off switch; `quota_mb` / `quota_hours`
  -- are the limit (NULL = no quota configured). The live window
  -- (start + bytes used) lives in the `quota_state` table below,
  -- not here, so a settings save never resets the window.
  quota_enabled INTEGER NOT NULL DEFAULT 0,
  quota_mb INTEGER,                 -- NULL = no quota configured
  quota_hours INTEGER,              -- NULL = no quota configured
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Download quota window (S12, 2026-11-15): singleton row id = 1.
-- `window_start_unix` is NULL until the first byte is counted after
-- the quota is enabled (or after a rollover / reset) — the window is
-- anchored at first use, not at a fixed epoch boundary. `bytes_used`
-- counts actual downloaded bytes (segment writes only — probe/HEAD
-- bytes never count). When `bytes_used` reaches the limit the queue
-- pauses; when the window rolls over the usage resets and
-- quota-paused downloads resume.
CREATE TABLE IF NOT EXISTS quota_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
  window_start_unix INTEGER,              -- NULL = no window open
  bytes_used INTEGER NOT NULL DEFAULT 0 CHECK (bytes_used >= 0)
);

-- Sounds tab (S09 finish, 2026-11-15): one row per event, a WAV
-- file or none, enabled or not. The engine emits the event and
-- plays the file on Windows only; on this Linux VM the event is
-- recorded with its resolved file and no audio is heard.
CREATE TABLE IF NOT EXISTS sound_settings (
  event TEXT PRIMARY KEY
    CHECK (event IN ('download_complete','download_failed','queue_finished')),
  enabled INTEGER NOT NULL DEFAULT 0,
  file_path TEXT,
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS licence (
  id INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
  owner_name TEXT,
  serial TEXT,
  status TEXT NOT NULL DEFAULT 'trial'
    CHECK (status IN ('trial','registered')),
  trial_expires_at TEXT,
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Browser-extension handoff (S13 / F01): links the native-messaging
-- host caught in the browser, waiting for the desktop app to pick
-- them up through the real add_download path. The host enqueues;
-- the engine drains. `download_id` is filled in when a row is
-- accepted; `error` when it is rejected. A row is never deleted by
-- the drain, so the history of what the browser sent stays visible.
CREATE TABLE IF NOT EXISTS pending_links (
  id TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  referrer TEXT,
  source TEXT NOT NULL DEFAULT 'browser'
    CHECK (source IN ('browser', 'download_all', 'video')),
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending', 'accepted', 'rejected')),
  download_id TEXT REFERENCES downloads(id) ON DELETE SET NULL,
  error TEXT,
  cookies TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Indexes: every foreign key + every filtered/sorted column
CREATE INDEX IF NOT EXISTS idx_downloads_status_created ON downloads(status, created_at);
CREATE INDEX IF NOT EXISTS idx_downloads_category ON downloads(category_id);
CREATE INDEX IF NOT EXISTS idx_downloads_queue ON downloads(queue_id);
CREATE INDEX IF NOT EXISTS idx_downloads_queue_position ON downloads(queue_id, queue_position);
CREATE INDEX IF NOT EXISTS idx_downloads_grabber ON downloads(grabber_project_id);
CREATE INDEX IF NOT EXISTS idx_segments_download ON segments(download_id, segment_index);
CREATE INDEX IF NOT EXISTS idx_pending_links_status_created ON pending_links(status, created_at);

-- Safety net: segments of one download must not overlap (SQLite has no
-- exclusion constraint, so a trigger enforces it).
CREATE TRIGGER IF NOT EXISTS trg_segments_no_overlap
BEFORE INSERT ON segments
BEGIN
  SELECT RAISE(ABORT, 'overlapping segment')
  WHERE NEW.end_byte IS NOT NULL AND EXISTS (
    SELECT 1 FROM segments
    WHERE download_id = NEW.download_id
      AND end_byte IS NOT NULL
      AND NEW.start_byte <= end_byte
      AND NEW.end_byte >= start_byte
  );
END;

-- Seed: builtin categories + main queue + singleton rows
INSERT OR IGNORE INTO categories (id, name, default_folder, file_extensions, is_builtin) VALUES
  ('cat-music',    'Music',    '', '["mp3","flac","wav","aac","ogg"]', 1),
  ('cat-video',    'Video',    '', '["mp4","mkv","avi","mov","webm"]', 1),
  ('cat-documents','Documents','', '["pdf","doc","docx","txt","epub"]', 1),
  ('cat-programs', 'Programs', '', '["exe","msi"]', 1),
  ('cat-compressed','Compressed','', '["zip","rar","7z","tar","gz"]', 1);

INSERT OR IGNORE INTO queues (id, name, type, is_main, schedule_type)
  VALUES ('queue-main', 'Main download queue', 'download', 1, 'one_time');

INSERT OR IGNORE INTO settings (id) VALUES (1);
INSERT OR IGNORE INTO quota_state (id, window_start_unix, bytes_used) VALUES (1, NULL, 0);
INSERT OR IGNORE INTO sound_settings (event, enabled, file_path) VALUES
  ('download_complete', 0, NULL),
  ('download_failed', 0, NULL),
  ('queue_finished', 0, NULL);
INSERT OR IGNORE INTO licence (id) VALUES (1);
