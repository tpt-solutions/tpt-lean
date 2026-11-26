# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Install tpt-lean and tlean from a GitHub Release (Windows x86_64).
#
#   powershell -ExecutionPolicy Bypass -File install.ps1 [-Version <tag>] [-Dir <path>] [-DryRun]
#
#   -Version   a release tag such as v0.1.0 or 0.1.0 (default: the latest release)
#   -Dir       install directory (default: $env:TPT_LEAN_INSTALL_DIR or %LOCALAPPDATA%\Programs\tpt-lean)
#   -DryRun    print what would be downloaded and where it would go; change nothing
#
# The archive is checked against SHA256SUMS from the same release before anything is extracted.
# Nothing is installed when the checksum does not match.

[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$Dir = $(if ($env:TPT_LEAN_INSTALL_DIR) { $env:TPT_LEAN_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\tpt-lean" }),
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$repo = if ($env:TPT_LEAN_REPO) { $env:TPT_LEAN_REPO } else { "tpt-solutions/tpt-lean" }
$base = "https://github.com/$repo/releases"

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "install.ps1: only 64-bit Windows is supported"
}
$target = "x86_64-pc-windows-msvc"
$archive = "tpt-lean-$target.zip"

if ($Version -eq "latest") {
    $releaseUrl = "$base/latest/download"
} else {
    $releaseUrl = "$base/download/v$($Version.TrimStart('v'))"
}

Write-Output "target:   $target"
Write-Output "archive:  $archive"
Write-Output "source:   $releaseUrl/$archive"
Write-Output "install:  $Dir"

if ($DryRun) {
    Write-Output "dry run: nothing was downloaded or installed"
    return
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("tpt-lean-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $archivePath = Join-Path $tmp $archive
    $sumsPath = Join-Path $tmp "SHA256SUMS"
    Invoke-WebRequest -Uri "$releaseUrl/$archive" -OutFile $archivePath -UseBasicParsing
    Invoke-WebRequest -Uri "$releaseUrl/SHA256SUMS" -OutFile $sumsPath -UseBasicParsing

    $expected = Get-Content $sumsPath | Where-Object { $_ -match [regex]::Escape($archive) + '$' } |
        ForEach-Object { ($_ -split '\s+')[0] } | Select-Object -First 1
    if (-not $expected) {
        throw "SHA256SUMS has no entry for $archive; nothing was installed"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -Path $archivePath).Hash
    if ($actual.ToLowerInvariant() -ne $expected.ToLowerInvariant()) {
        throw "checksum mismatch for $archive; nothing was installed"
    }
    Write-Output "checksum: ok"

    $extract = Join-Path $tmp "extract"
    Expand-Archive -Path $archivePath -DestinationPath $extract
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    foreach ($bin in @("tpt-lean.exe", "tlean.exe")) {
        $src = Join-Path $extract $bin
        if (-not (Test-Path $src)) { throw "archive does not contain $bin" }
        Copy-Item -Force $src (Join-Path $Dir $bin)
    }
    Write-Output "installed tpt-lean and tlean to $Dir"

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($userPath -split ';') -notcontains $Dir) {
        Write-Output "add $Dir to your PATH (for example: [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$Dir`", 'User'))"
    }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
