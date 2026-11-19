# Third-Party Notices — Sonic Manager

Sonic Manager bundles the following third-party binaries for platform
video downloads (YouTube, Facebook, Instagram, TikTok). Each is a
separate program, run as its own child process — none of their code is
linked into Sonic Manager.

> Packaging note (Round 78): the binaries are fetched at
> build/packaging time, NOT checked into the repository. When a
> release is packaged, fill in the exact version and SHA-256 of the
> file that was actually bundled, and verify the checksum before
> signing. The `version` / `SHA-256` fields below are the slots for
> that — they must not ship blank.

## yt-dlp

- **What:** downloads and resolves video pages (formats list, direct
  media URLs, fragments). Sonic Manager's engine downloads the media
  itself; yt-dlp only resolves, and is the direct downloader for
  fragmented/manifest formats (the permanent fallback).
- **Version:** _fill at packaging — the exact release tag bundled_
- **License:** The Unlicense (public domain)
- **Source:** https://github.com/yt-dlp/yt-dlp/releases
- **SHA-256 (`yt-dlp.exe`):** _fill at packaging_
- **Update path:** Settings → Video downloads → "Update yt-dlp"
  downloads the newest release from the same page and verifies it
  against the release's published `SHA2-256SUMS` before installing.

## QuickJS-ng (`qjs.exe`)

- **What:** the JavaScript runtime yt-dlp uses to solve YouTube's
  signature challenges (`--js-runtimes quickjs:<path>`). ~1.3–2 MB.
- **Version:** _fill at packaging — the exact official Windows x86_64 release_
- **License:** MIT
- **Source:** https://github.com/quickjs-ng/quickjs/releases
- **SHA-256 (`qjs.exe`):** _fill at packaging_

## FFmpeg (`ffmpeg.exe`, `ffprobe.exe` + DLLs)

- **What:** merges separate video and audio streams into one file
  (stream copy, `-c copy` — no re-encoding). yt-dlp's merge needs
  BOTH `ffmpeg.exe` and `ffprobe.exe` ("Required for merging
  separate video and audio files" — yt-dlp README); a merge with
  only ffmpeg fails with "ffprobe and ffmpeg not found".
- **Build:** BtbN/FFmpeg-Builds, PINNED release branch (n8.1 or
  n9.0), **win64-lgpl-shared**. Only `ffmpeg.exe`, `ffprobe.exe`
  and the DLLs they need are shipped — never `ffplay`.
- **Version:** _fill at packaging — the exact BtbN release tag + branch_
- **License:** GNU Lesser General Public License (LGPL) — the
  shared build contains no GPL components. FFmpeg's own
  `LICENSE.txt` ships next to the binaries. The build must be
  verified with `ffmpeg -L` at packaging: reject any build
  listing `--enable-gpl`, `--enable-nonfree` or `fdk-aac`.
- **Source:** https://github.com/BtbN/FFmpeg-Builds/releases
- **SHA-256 (`ffmpeg.exe` / `ffprobe.exe` / each DLL):** _fill at packaging_

> NEVER bundle gyan.dev FFmpeg builds: every gyan.dev build is GPL,
> and bundling GPL ffmpeg into this non-GPL program is a licensing
> problem. BtbN LGPL-shared is the only approved source.

## What is NOT bundled

- **Deno** — considered and rejected (QuickJS-ng is ~1.3–2 MB vs
  ~110+ MB uncompressed; QuickJS is gated by a VPS acceptance test
  of ~20 varied YouTube URLs — zero solver failures or Deno wins).
- **yt-dlp-ejs** — the JavaScript challenge solver yt-dlp ships
  (pure ES2020/ES2023); it is part of yt-dlp's own distribution,
  not a separate binary.
