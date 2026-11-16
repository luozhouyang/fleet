#!/usr/bin/env pwsh
# Sign Windows release binaries with Authenticode.
#
# Usage:
#   scripts/release/sign-windows.ps1 target/release/fleet.exe target/release/fleet-admin.exe target/release/fleet-daemon.exe
#
# Required environment when signing is enabled:
#   WINDOWS_CERTIFICATE_PFX       base64-encoded code-signing .pfx (with private key)
#   WINDOWS_CERTIFICATE_PASSWORD  password used when exporting the .pfx
#
# Optional:
#   WINDOWS_TIMESTAMP_URL         RFC 3161 timestamp server
#                                 (default: http://timestamp.digicert.com)
#   REQUIRE_WINDOWS_SIGNING       fail instead of skipping when the .pfx is absent
#
# Without WINDOWS_CERTIFICATE_PFX the script prints a notice and exits 0, so
# unsigned development releases still work.
[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $Binaries
)

$ErrorActionPreference = "Stop"

if (-not $env:WINDOWS_CERTIFICATE_PFX) {
    if ($env:REQUIRE_WINDOWS_SIGNING) {
        Write-Error "REQUIRE_WINDOWS_SIGNING is set but WINDOWS_CERTIFICATE_PFX is empty"
        exit 1
    }
    Write-Host "notice: WINDOWS_CERTIFICATE_PFX is not set; skipping Windows signing"
    exit 0
}

if (-not $env:WINDOWS_CERTIFICATE_PASSWORD) {
    Write-Error "WINDOWS_CERTIFICATE_PASSWORD is required when WINDOWS_CERTIFICATE_PFX is set"
    exit 1
}

if (-not $Binaries -or $Binaries.Count -eq 0) {
    Write-Error "usage: sign-windows.ps1 <exe> [exe...]"
    exit 2
}

$signtool = $null
$command = Get-Command signtool.exe -ErrorAction SilentlyContinue
if ($command) {
    $signtool = $command.Source
}
if (-not $signtool) {
    $kits = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    $candidate = Get-ChildItem -Path $kits -Filter signtool.exe -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if ($candidate) {
        $signtool = $candidate.FullName
    }
}
if (-not $signtool) {
    Write-Error "signtool.exe not found; install the Windows SDK"
    exit 1
}

$timestamp = if ($env:WINDOWS_TIMESTAMP_URL) {
    $env:WINDOWS_TIMESTAMP_URL
} else {
    "http://timestamp.digicert.com"
}

$tempRoot = if ($env:RUNNER_TEMP) {
    $env:RUNNER_TEMP
} else {
    [IO.Path]::GetTempPath()
}
$tmp = Join-Path $tempRoot ("fleet-signing-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
$pfx = Join-Path $tmp "certificate.pfx"

try {
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE_PFX))

    foreach ($file in $Binaries) {
        if (-not (Test-Path -LiteralPath $file)) {
            throw "binary not found: $file"
        }
        Write-Host "Signing $file"
        & $signtool sign /fd SHA256 /td SHA256 /tr $timestamp /f $pfx /p $env:WINDOWS_CERTIFICATE_PASSWORD /v $file
        if ($LASTEXITCODE -ne 0) {
            throw "signtool sign failed for $file"
        }

        & $signtool verify /pa /all /v $file
        if ($LASTEXITCODE -ne 0) {
            throw "signtool verify failed for $file"
        }
    }
}
finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
