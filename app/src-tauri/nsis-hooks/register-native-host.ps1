# Fetchline native-messaging host manifest fix-up (NSIS POSTINSTALL).
#
# The installer ships the two host manifests with the default MSI
# path (C:\Program Files\Fetchline\fetchline-host.exe). The default
# NSIS install is per-user under %LOCALAPPDATA% (and the user may
# pick any folder), so the "path" field must point at the real
# install dir or Chrome/Firefox can never spawn the host.
#
# Usage: register-native-host.ps1 <install-dir>
# Rewrites the "path" field of both manifests in place and writes
# them back as UTF-8 WITHOUT a BOM (Chrome rejects a BOM-prefixed
# manifest). Exits non-zero on failure; the NSIS hook prints a
# warning and continues (the CI capture test is the safety net).
param([Parameter(Mandatory = $true)][string]$InstallDir)

$ErrorActionPreference = 'Stop'
$hostExe = Join-Path $InstallDir 'fetchline-host.exe'
foreach ($name in 'com.fetchline.host.json', 'com.fetchline.host.firefox.json') {
    $path = Join-Path $InstallDir $name
    $manifest = [System.IO.File]::ReadAllText($path) | ConvertFrom-Json
    $manifest.path = $hostExe
    [System.IO.File]::WriteAllText(
        $path,
        ($manifest | ConvertTo-Json),
        [System.Text.UTF8Encoding]::new($false)
    )
}
