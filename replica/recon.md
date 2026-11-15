# Recon map: Internet Download Manager (IDM) — Windows desktop

Scope: Windows desktop download manager. Core slice: the download loop —
catch a link, segment + accelerate it, pause/resume it, queue/schedule it,
organise it by category. (Full IDM, not one feature.)
For: the user, as a clean-room rebuild of functionality and UX patterns only.
Date: 2026-11-14

## Sources

| # | source | URL | notes |
| --- | --- | --- | --- |
| 1 | Official features page | https://www.internetdownloadmanager.com/features2.html | Speed, segmentation, resume, scheduler, grabber, categories, dark theme, proxy/auth |
| 2 | Official home page | https://www.internetdownloadmanager.com/ | Positioning: "accelerate downloads by up to 8 times, resume, organize and schedule" |
| 3 | Official Scheduler help | https://www.internetdownloadmanager.com/support/idm-scheduler/idm_scheduler.html | Queue dialog, one-time downloading vs periodic sync, start/stop, run file on completion |
| 4 | Official Options help | https://www.internetdownloadmanager.com/support/using_idm/options.html | Options dialog, 9 tabs: General, File Types, Connection, Save To, Downloads, Proxy/Socks, Site logins, Dial-Up, Sounds |
| 5 | Official start / welcome page | https://www.internetdownloadmanager.com/welcome.html | "Download File Info" dialog, Download later -> main queue, grabber wizard |
| 6 | Step-by-step guide (3rd party) | https://tutorials.mediaket.net/software-tutorials/internet-download-manager-tutorial.html | Add URL, categories, scheduler, batch download, site grabber, speed limiter, max connections 8-16 |

## Core loop

A browser link is caught by IDM, split into dynamic segments downloaded in
parallel, then the finished file lands in the right category folder — and any
interrupted download can be resumed from where it stopped.

## Screens

| ID | screen | route / how to reach | purpose | key components | states seen |
| --- | --- | --- | --- | --- | --- |
| S01 | Main window | Launch IDM | The download list and control centre | Toolbar (Add URL, Resume, Stop, Stop All, Delete, Options, Scheduler, Categories), left Categories tree, main download table (Name, Size, Status, Speed, Time left, Category), status bar | empty (no downloads), filled, downloading, paused, complete, error |
| S02 | Download File Info dialog | Browser click on a link, or Tasks > Add new download / Add URL | Confirm one download before it starts | URL field, filename, save-to folder + browse, category dropdown, description, Start Download / Download Later buttons | new URL, pre-filled from browser, invalid URL, duplicate file exists |
| S03 | Download progress dialog | While a download runs (or double-click a row) | Live progress for one file | Progress bar, speed graph, segments view, downloaded/total size, time left, Pause / Resume / Cancel, "Open folder" on completion | downloading, paused, complete, error / connection lost, waiting for retry |
| S04 | Categories pane | Left side of S01; Tasks > Add Category | Organise downloads by type | Tree: All Downloads, Unfinished, Finished, Queues; per-category folders (Music, Video, Documents, Programs, Compressed) | empty category, filled, custom category added |
| S05 | Scheduler / Queues dialog | Scheduler toolbar button, or Downloads > Scheduler | Run queues at a time, or sync them periodically | Queue list (left), Schedule tab (one-time downloading / periodic synchronisation, start/stop time, start on startup), Files in Queue tab, Start Now, download limits, on-completion action (open file / run program / shut down / hibernate) | no queue, queue with files, running, finished, sync queue |
| S06 | Site Grabber wizard | Tasks > Run site grabber | Download a whole site, or a filtered subset, for offline browsing | Steps: project name + start URL, file-type filters, explore depth, download / explore only, scheduler for the project | new project, exploring, downloading, finished, 2-attempt failure (grabber stops after 2 attempts) |
| S07 | Download All dialog | Browser right-click > Download all links with IDM, or "Download All" | Add every link on the current page at once | Checklist of links (URL, type, size), select all / none, filter by type, category, Start / Later | page with many links, page with no downloadable links, filtered list |
| S08 | Video grabber / browser video panel | Browser extension overlay on a web video player | Offer to download an embedded video/audio in several qualities | "Download this video" floating button, quality/format list (MP4, FLV, WebM, MP3), download / later | video detected, no video on page, multiple qualities, live stream |
| S09 | Options dialog | Options toolbar button (gear) | All settings | 9 tabs: General (browser integration, default folder), File Types (auto-start extensions, excluded sites), Connection (max connections, speed limiter, exceptions), Save To (per-category folders), Downloads (dialogs, virus check, download limits), Proxy / Socks, Site logins (saved credentials), Dial-Up, Sounds | default, changed-not-saved, invalid proxy |
| S10 | Add batch download dialog | Tasks > Add batch download | Add many URLs in one go | Multi-line URL box (one per line), category, Start | empty box, invalid lines mixed with valid ones, large list |
| S11 | Registration / licence dialog | Help > Registration, or trial expiry | Activate a licence | Name, serial number, Register, trial countdown (30-day trial) | trial active, trial expired, invalid serial, registered |
| S12 | Download limits / Speed limiter | Downloads > Speed Limiter; Options > Connection > Download limits | Cap bandwidth / quota | Speed limit value (KB/s), quota: X MB every Y hours, enable/disable | unlimited, limited, quota reached |
| S13 | Browser integration (extension) | Browser extension install; context menu | Catch downloads from the browser | "Download with IDM" context-menu item, extension popup, per-site on/off | extension installed, extension missing, site excluded |
| S14 | Completion / antivirus dialog | On download finish | Confirm the file and scan it | File path, Open / Open folder, antivirus scan result / run scanner | clean, scanner not configured, scan failed |

## Flows

```
F01 One-click browser download (the core loop)
    Browser link click -> S13 catches it -> S02 Download File Info -> S03 progress -> S01 row = Finished
    happy path clicks: 2 (click link, Start Download)
    edge: server does not support resume, file already exists, browser extension missing, excluded site/file type

F02 Manual Add URL
    S01 Add URL -> S02 paste URL -> Start Download / Download Later -> S03 / S05 queue
    happy path clicks: 3
    edge: invalid URL, URL needs login (S09 Site logins), redirect, 404

F03 Pause / resume an interrupted download
    S01/S03 Pause or connection drops -> S01 status = Paused/Error -> Resume -> S03 continues from last byte
    happy path clicks: 1 (Resume)
    edge: server dropped resume support, file changed on server, computer shut down / power outage mid-download

F04 Queue + schedule a download
    S02 Download Later -> S05 pick queue -> set start/stop time (one-time) -> Start Now or wait -> queue drains, files removed from queue when done
    happy path clicks: 4
    edge: start time passed, stop time before finish, queue contains a failed file, shutdown-on-completion

F05 Capture a video from a web page
    Play video in browser -> S08 floating button appears -> pick quality/format -> S02 -> S03
    happy path clicks: 3
    edge: video is DRM-protected / no downloadable stream, live stream, only low quality available, site blocks the grabber

F06 Site grabber: download a site / gallery
    Tasks > Run site grabber -> S06 wizard (URL, filters, depth) -> explore -> download matched files -> S01
    happy path clicks: 5
    edge: site blocks crawling, huge site (depth too high), only 2 attempts per file then grabber gives up, login-required pages

F07 Download all links on a page
    Browser right-click > Download all links -> S07 checklist -> select -> Start
    happy path clicks: 3
    edge: page has no links, duplicate links, mixed file types

F08 Batch download from a URL list
    Tasks > Add batch download -> S10 paste URLs -> Start -> S01 rows appear
    happy path clicks: 3
    edge: blank/invalid lines, one URL fails while others succeed

F09 Configure connections, proxy and logins
    S09 Options -> Connection tab (max connections, exceptions) -> Proxy/Socks tab -> Site logins tab -> Save
    happy path clicks: 5
    edge: proxy needs auth (Basic / NTLM / Kerberos), site in exceptions, wrong credentials

F10 Organise into categories
    S01 drag file to a category in S04, or right-click > Move to Category; Tasks > Add Category for a custom one
    happy path clicks: 1 (drag) / 3 (menu)
    edge: category folder missing, file type does not match the category rule
```

## Components

| component | variants | states | used on |
| --- | --- | --- | --- |
| Toolbar button | icon + label, icon only | default, hover, disabled, active | S01 |
| Download table row | — | downloading, paused, complete, error, queued | S01 |
| Progress bar + speed graph | thin, dialog | 0-100%, stalled, complete | S01, S03 |
| Modal dialog | small (confirm), large (Options, Scheduler) | default, error, loading | S02, S05, S06, S09, S10, S11 |
| Categories tree | folder, queue | collapsed, expanded, selected | S01, S04, S05 |
| Dropdown | category, quality, format | default, open, disabled | S02, S07, S08 |
| Checkbox list | links, files in queue | checked, unchecked, partial | S05, S07 |
| Tabs | Options tabs, Scheduler tabs | active, inactive | S05, S09 |
| Toast / status bar | info, error | transient | S01, S03 |
| Floating browser button | video download button | visible on video hover, hidden | S08 |
| Text input + browse | URL, folder path | default, invalid, focused | S02, S06, S09 |
| Toggle / radio | one-time vs periodic sync, dark theme | on, off | S05, S09 |

## Inferred data model

```
DownloadItem  id, url, final_url, filename, save_path, category_id, queue_id,
              size_bytes (nullable), downloaded_bytes, status
              (queued | downloading | paused | complete | error | deleted),
              speed_bps, eta_seconds, created_at, completed_at, referrer,
              cookies / headers (json, nullable), error_message (nullable)
              evidence: S01 table columns, S02 fields, S03 progress
              confidence: high

Segment  id, download_id, index, start_byte, end_byte, downloaded_bytes,
         status (pending | active | done | failed), connection_id
         evidence: official "dynamic segmentation" — segments are split
         during the download, largest segment halved when a new connection
         opens; S03 segment view
         confidence: high (structure), medium (exact split rule)

Queue  id, name, type (download | synchronisation), is_main,
       schedule_type (one_time | periodic), start_at, stop_at,
       start_on_startup (bool), on_complete_action
       (none | open_file | run_program | shutdown | hibernate),
       download_limit_mb_per_hours (nullable)
       evidence: S05 Scheduler dialog, official scheduler help
       confidence: high

Category  id, name, default_folder, file_extensions (list), is_builtin
          evidence: S04 tree, S09 Save To tab, "organize downloads
          automatically using defined download categories"
          confidence: high

GrabberProject  id, name, start_url, filters (file types, depth),
                mode (explore | download), schedule (nullable),
                status (idle | exploring | downloading | done | failed)
                evidence: S06 wizard, official grabber help
                confidence: high

SiteLogin  id, host, username, password_ref, auth_type
           (basic | negotiate | ntlm | kerberos)
           evidence: S09 Site logins tab, official "main authentication protocols"
           confidence: medium (password storage format is a guess)

ProxyConfig  id, type (http | ftp | socks), host, port, username, password_ref,
             use_for (list)
             evidence: S09 Proxy / Socks tab
             confidence: medium

Settings  id (singleton), default_download_folder, max_connections,
          speed_limit_kbps (nullable), download_limit, language, theme
          (light | dark), browser_integration (bool), antivirus_command
          (nullable), file_types_auto (list), excluded_sites (list),
          check_updates_weekly (bool)
          evidence: S09 Options tabs, official features page
          confidence: high

Licence  id (singleton), owner_name, serial, status (trial | registered),
         trial_expires_at
         evidence: S11, 30-day trial on official site
         confidence: high
```

Relationships: Category 1-n DownloadItem, Queue 1-n DownloadItem,
DownloadItem 1-n Segment, GrabberProject 1-n DownloadItem,
Settings 1-1 Licence (singleton app state).

## Feature matrix

See `features.csv`. Must: 18, should: 9, could: 5, skip: 4.

## Out of scope (cannot or should not be cloned)

- IDM name, globe logo, skins, copy and marketing text — the brand is theirs.
  The rebuild needs its own name, palette and wording (replica-brand).
- The exact proprietary segmentation / acceleration internals — rebuild the
  behaviour (HTTP Range requests, dynamic splitting) from public knowledge,
  do not copy their code or binaries.
- Downloading DRM-protected streams, or bypassing a site's paywall / login /
  terms to grab media. Video capture is in scope only for openly
  downloadable streams, and only where the site allows it.
- Their browser-extension store listings and the "Advanced Browser
  Integration" hook that catches downloads from any application — rebuilding
  the extension is possible, hooking arbitrary apps is not a sane v1.
- Antivirus engines themselves (Avast, Norton, etc.) — only the "run a
  scanner on completion" hook is in scope.
- The existing user base / network — there is none to clone; IDM is a local
  desktop app, so this is not a network problem.

## Size

Screens 14, flows 10, entities 9. Hard parts: (1) multi-connection segmented
downloading with dynamic re-splitting and reliable resume; (2) browser
extension + video detection across Chrome/Firefox/Edge; (3) site grabber /
spider with filters, depth and politeness; (4) Windows-native integration
(tray, drag-and-drop, context menu, shutdown-on-completion).
Size: L (a quarter) for the full feature set; M (a few weeks) for the core
MVP — F01 + F02 + F03 + F04 with S01/S02/S03/S05 and no grabber.
