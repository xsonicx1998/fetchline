# Fetchline

[![Windows build](https://github.com/mohandksonic-ui/fetchline/actions/workflows/build-windows.yml/badge.svg)](https://github.com/mohandksonic-ui/fetchline/actions/workflows/build-windows.yml)

> **The name "Fetchline" is provisional** — a temporary working name while the
> product is being built. It will be renamed before release.

Fetchline is a Windows desktop download manager — a clean-room rebuild of
download-manager behaviour, written from scratch. No code, assets, copy or
branding from any other download manager is used anywhere in this project.

- **Stack:** Tauri 2 + Rust engine (tokio + reqwest) + TypeScript/React UI + SQLite
- **Features:** segmented downloads with pause/resume, queues and scheduler,
  categories, Download All / batch, browser extension, video grabber,
  site grabber, proxy support (HTTP/FTP/SOCKS5, Basic/NTLM/Kerberos),
  speed limiter + download quota, update check, antivirus-scan hook
- **Docs:** the full specification lives in [`replica/`](replica/) —
  `recon.md` (screens/flows), `architecture.md`, `features.csv` (feature
  matrix), `build-log.md`, `parity.md`, `bugs.md`, `test-plan.md`

## Download (Windows, no setup needed)

Every push to `main` builds a Windows installer automatically:

1. Open the [**Actions** tab](https://github.com/mohandksonic-ui/fetchline/actions)
   and pick the latest successful run of **Build Windows installer**.
2. Under **Artifacts**, download `fetchline-windows-installer`.
3. Run the `.msi` (or the NSIS `.exe`) inside and install.

No Rust, Node or build tools are needed on your machine — the build happens
on GitHub's Windows runners.

## Build from source (Windows)

```powershell
# Prerequisites: Rust (stable, via rustup), Node.js 20+, Visual Studio
# Build Tools (C++ workload). WebView2 is already on Windows 10/11.
cd app
npm ci
npm run tauri build
# Installer lands in app/src-tauri/target/release/bundle/
```

## Develop (Linux)

The Rust engine and its tests run on Linux; the Tauri shell itself does not
compile here (missing `gdk-3.0`), so there is no Linux desktop binary.

```sh
cd app/engine && cargo test        # engine test suite
cd app && npm run build            # frontend build
python3 scripts/generate-icons.py  # regenerate icon PNGs (binary icons
                                   # are not stored in the repo)
```

See [`app/README.md`](app/README.md) for the full feature and layout notes.
