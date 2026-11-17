#requires -Version 5.1
<#
.SYNOPSIS
    Install Fleet on Windows for the current user.

.DESCRIPTION
    Downloads the Fleet release archive for this machine, verifies it against
    the release SHA256SUMS file, extracts it to a temporary directory, and runs
    `fleet.exe install --from-dir <temp>` with the requested options.

    Run this script from a normal, non-elevated PowerShell prompt. The script
    refuses to run from an elevated session. `fleet install` owns the per-user
    installation and requests elevation once, only for Task Scheduler service
    registration; this bootstrapper never registers services itself.

.PARAMETER Version
    Release to install: `latest` (default), `v1.2.3`, or `1.2.3`.

.PARAMETER Prefix
    Install prefix. Default: %LOCALAPPDATA%\Fleet. Forwarded to `fleet install`.

.PARAMETER Port
    Admin HTTP port. Default: 8787. Forwarded to `fleet install`.

.PARAMETER Yes
    Accept prompts in `fleet install`.

.PARAMETER DryRun
    Print the resolved release, architecture, prefix, download URLs, and the
    forwarded command, then exit without downloading or writing anything.

.PARAMETER Force
    Reinstall over an existing installation.

.EXAMPLE
    .\install.ps1

.EXAMPLE
    .\install.ps1 -Version v0.1.0 -Port 8787 -Yes

.EXAMPLE
    .\install.ps1 -DryRun

.NOTES
    Environment:
      FLEET_BASE_URL  Override the release download base (mirrors and tests).
                      Must be an absolute http(s) directory URL. When set, it
                      takes precedence over -Version for URL resolution.
#>
[CmdletBinding()]
param(
    [string]$Version = "latest",

    [string]$Prefix,

    [ValidateRange(1, 65535)]
    [int]$Port = 8787,

    [switch]$Yes,

    [switch]$DryRun,

    [switch]$Force,

    # Internal, test-only escape hatch. Not part of the supported installer
    # contract; it is intentionally omitted from the help text above.
    [switch]$NoService
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
# PowerShell 7.3+ can turn a native nonzero exit into a terminating error.
# Keep native failures as exit codes so fleet install's code is propagated.
$PSNativeCommandUseErrorActionPreference = $false

$Repo = "luozhouyang/fleet"

function Write-Failure {
    param([string]$Message)
    [Console]::Error.WriteLine("error: {0}" -f $Message)
}

function Resolve-FleetArchitecture {
    $arch = $null
    try {
        $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    } catch {
        $arch = $null
    }
    if ([string]::IsNullOrWhiteSpace($arch)) {
        $arch = [System.Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITEW6432")
    }
    if ([string]::IsNullOrWhiteSpace($arch)) {
        $arch = [System.Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITECTURE")
    }
    if ([string]::IsNullOrWhiteSpace($arch)) {
        throw "cannot determine the Windows architecture"
    }

    switch ($arch.Trim().ToUpperInvariant()) {
        "AMD64" { return "x86_64" }
        "X64" { return "x86_64" }
        "X86_64" { return "x86_64" }
        "ARM64" { return "arm64" }
        "AARCH64" { return "arm64" }
        default {
            throw "unsupported architecture '$arch'; Fleet publishes x86_64 and arm64 Windows builds"
        }
    }
}

function Resolve-ReleaseBase {
    param([string]$RequestedVersion)

    $override = [System.Environment]::GetEnvironmentVariable("FLEET_BASE_URL")
    if (-not [string]::IsNullOrWhiteSpace($override)) {
        $candidate = $override.Trim().TrimEnd("/")
        try {
            $uri = [System.Uri]$candidate
        } catch {
            throw "FLEET_BASE_URL is not a valid absolute http(s) directory URL"
        }
        if (-not $uri.IsAbsoluteUri -or [string]::IsNullOrEmpty($uri.Host) -or @("http", "https") -notcontains $uri.Scheme.ToLowerInvariant()) {
            throw "FLEET_BASE_URL must be an absolute http(s) URL"
        }
        if (-not [string]::IsNullOrEmpty($uri.UserInfo)) {
            throw "FLEET_BASE_URL must not embed credentials; use a credential-free mirror URL"
        }
        if (-not [string]::IsNullOrEmpty($uri.Query) -or -not [string]::IsNullOrEmpty($uri.Fragment)) {
            throw "FLEET_BASE_URL must not contain a query string or fragment"
        }
        return $candidate
    }

    if ($RequestedVersion -eq "latest") {
        return "https://github.com/$Repo/releases/latest/download"
    }
    if ($RequestedVersion -match '^v') {
        return "https://github.com/$Repo/releases/download/$RequestedVersion"
    }
    return "https://github.com/$Repo/releases/download/v$RequestedVersion"
}

function Resolve-InstallPrefix {
    param([string]$RequestedPrefix)

    if (-not [string]::IsNullOrWhiteSpace($RequestedPrefix)) {
        $expanded = [System.Environment]::ExpandEnvironmentVariables($RequestedPrefix)
        try {
            $resolved = [System.IO.Path]::GetFullPath($expanded)
        } catch {
            throw "invalid -Prefix: $($_.Exception.Message)"
        }
        # Avoid trailing-separator quoting surprises when the path is passed to
        # a native command. Keep the separator at a drive or UNC share root.
        $root = [System.IO.Path]::GetPathRoot($resolved)
        if ($resolved.Length -gt $root.Length) {
            $resolved = $resolved.TrimEnd([char]'\', [char]'/')
        }
        return $resolved
    }

    $localAppData = [System.Environment]::GetEnvironmentVariable("LOCALAPPDATA")
    if ([string]::IsNullOrWhiteSpace($localAppData)) {
        throw "LOCALAPPDATA is not set; pass -Prefix explicitly"
    }
    return [System.IO.Path]::GetFullPath((Join-Path $localAppData "Fleet"))
}

# Strip credentials, query strings, and fragments from URLs before printing.
function Get-DisplayUrl {
    param([string]$Url)
    try {
        $uri = [System.Uri]$Url
        if (-not $uri.IsAbsoluteUri) {
            return "<invalid URL>"
        }
        return "{0}://{1}{2}" -f $uri.Scheme, $uri.Authority, $uri.AbsolutePath
    } catch {
        return "<invalid URL>"
    }
}

function Save-ReleaseFile {
    param(
        [string]$Uri,
        [string]$OutFile,
        [string]$Description
    )

    try {
        Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing -ErrorAction Stop
    } catch {
        $downloadError = $_
        $statusCode = $null
        $exception = $downloadError.Exception
        while ($null -ne $exception) {
            try {
                if ($null -ne $exception.Response) {
                    $statusCode = [int]$exception.Response.StatusCode
                    break
                }
            } catch {
                $statusCode = $null
            }
            $exception = $exception.InnerException
        }

        $displayUri = Get-DisplayUrl -Url $Uri
        if ($statusCode -eq 404) {
            throw "$Description was not found at $displayUri. The release may not publish this file."
        }

        $detail = $downloadError.Exception.Message
        if (-not [string]::IsNullOrEmpty($Uri)) {
            $detail = $detail.Replace($Uri, $displayUri)
        }
        throw "failed to download ${Description} from ${displayUri}: $detail"
    }
}

function Get-ExpectedChecksum {
    param(
        [string]$SumsPath,
        [string]$AssetName
    )

    $pattern = '^\s*([0-9A-Fa-f]{64})\s+\*?(.+?)\s*$'
    foreach ($line in (Get-Content -LiteralPath $SumsPath -ErrorAction Stop)) {
        if ($line -match $pattern) {
            $name = $Matches[2]
            if ([string]::Equals($name, $AssetName, [System.StringComparison]::OrdinalIgnoreCase)) {
                return $Matches[1].ToLowerInvariant()
            }
        }
    }
    return $null
}

function Format-CommandLine {
    param([string[]]$Arguments)

    $rendered = foreach ($argument in $Arguments) {
        if ($argument -match '[\s"]') {
            '"' + $argument.Replace('"', '\"') + '"'
        } else {
            $argument
        }
    }
    return ($rendered -join " ")
}

function Write-InstallPlan {
    param(
        [string]$Release,
        [string]$Architecture,
        [string]$InstallPrefix,
        [string]$AssetName,
        [string]$AssetUrl,
        [string]$SumsUrl,
        [string]$CommandPreview
    )

    Write-Host "Fleet installer for Windows"
    Write-Host ("  release:   {0}" -f $Release)
    Write-Host ("  platform:  windows/{0}" -f $Architecture)
    Write-Host ("  prefix:    {0}" -f $InstallPrefix)
    Write-Host ("  asset:     {0}" -f $AssetName)
    Write-Host ("  source:    {0}" -f (Get-DisplayUrl -Url $AssetUrl))
    Write-Host ("  checksums: {0}" -f (Get-DisplayUrl -Url $SumsUrl))
    Write-Host ("  install:   {0}" -f $CommandPreview)
}

if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    Write-Failure "install.ps1 supports Windows only; on Linux, macOS, or WSL use install.sh."
    exit 1
}

$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object -TypeName System.Security.Principal.WindowsPrincipal -ArgumentList $identity
if ($principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Failure "do not run this installer from an elevated prompt. Run it as your normal Windows account; 'fleet install' requests elevation only for Task Scheduler service registration."
    exit 1
}

try {
    $currentProtocol = [System.Net.ServicePointManager]::SecurityProtocol
    [System.Net.ServicePointManager]::SecurityProtocol = $currentProtocol -bor [System.Net.SecurityProtocolType]::Tls12
} catch {
    # Older runtimes may not expose Tls12; the download reports the real error.
}

if ([string]::IsNullOrWhiteSpace($Version) -or $Version -notmatch '^[A-Za-z0-9][A-Za-z0-9._+-]*$') {
    Write-Failure "invalid -Version '$Version'; use latest, v1.2.3, or 1.2.3."
    exit 1
}

$tempRoot = $null
$exitCode = 0
try {
    $arch = Resolve-FleetArchitecture
    $installPrefix = Resolve-InstallPrefix -RequestedPrefix $Prefix
    $releaseBase = Resolve-ReleaseBase -RequestedVersion $Version
    $assetName = "fleet-windows-$arch.zip"
    $assetUrl = "$releaseBase/$assetName"
    $sumsUrl = "$releaseBase/SHA256SUMS"

    $previewArguments = @(
        "--from-dir", "<temp>",
        "--prefix", $installPrefix,
        "--port", [string]$Port
    )
    if ($Yes) { $previewArguments += "--yes" }
    if ($Force) { $previewArguments += "--force" }
    if ($NoService) { $previewArguments += "--no-service" }
    $commandPreview = "fleet.exe install " + (Format-CommandLine -Arguments $previewArguments)

    Write-InstallPlan `
        -Release $Version `
        -Architecture $arch `
        -InstallPrefix $installPrefix `
        -AssetName $assetName `
        -AssetUrl $assetUrl `
        -SumsUrl $sumsUrl `
        -CommandPreview $commandPreview

    if ($DryRun) {
        Write-Host "dry run: nothing was downloaded or written"
    } else {
        if ($null -eq (Get-Command Get-FileHash -ErrorAction SilentlyContinue)) {
            throw "Get-FileHash is required; use Windows PowerShell 5.1 or PowerShell 7."
        }

        $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("fleet-install-" + [System.Guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $tempRoot -ErrorAction Stop | Out-Null

        $downloadDir = Join-Path $tempRoot "download"
        $extractDir = Join-Path $tempRoot "unpack"
        New-Item -ItemType Directory -Path $downloadDir -ErrorAction Stop | Out-Null

        $zipPath = Join-Path $downloadDir $assetName
        $sumsPath = Join-Path $downloadDir "SHA256SUMS"

        Write-Host "Downloading SHA256SUMS"
        Save-ReleaseFile -Uri $sumsUrl -OutFile $sumsPath -Description "SHA256SUMS"

        Write-Host "Downloading $assetName"
        Save-ReleaseFile -Uri $assetUrl -OutFile $zipPath -Description $assetName

        $expected = Get-ExpectedChecksum -SumsPath $sumsPath -AssetName $assetName
        if ($null -eq $expected) {
            throw "SHA256SUMS does not contain an entry for $assetName; refusing to install an unverified archive"
        }
        $actual = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
        if (-not [string]::Equals($expected, $actual, [System.StringComparison]::Ordinal)) {
            throw "checksum mismatch for ${assetName}: expected $expected, got $actual"
        }
        Write-Host "Verified SHA256 $actual"

        Write-Host "Extracting $assetName"
        Expand-Archive -LiteralPath $zipPath -DestinationPath $extractDir -Force -ErrorAction Stop

        $fleetExe = Join-Path $extractDir "bin\fleet.exe"
        if (-not (Test-Path -LiteralPath $fleetExe -PathType Leaf)) {
            throw "unexpected archive layout: bin\fleet.exe was not found after extraction"
        }

        $installArguments = @(
            "install",
            "--from-dir", $extractDir,
            "--prefix", $installPrefix,
            "--port", [string]$Port
        )
        if ($Yes) { $installArguments += "--yes" }
        if ($Force) { $installArguments += "--force" }
        if ($NoService) { $installArguments += "--no-service" }

        Write-Host ("Running {0}" -f (Format-CommandLine -Arguments (@($fleetExe) + $installArguments)))
        & $fleetExe @installArguments
        if ($null -eq $LASTEXITCODE) {
            throw "could not read the exit code from fleet install"
        }
        $exitCode = [int]$LASTEXITCODE
        if ($exitCode -ne 0) {
            Write-Failure "fleet install exited with code $exitCode"
        }
    }
} catch {
    Write-Failure $_.Exception.Message
    $exitCode = 1
} finally {
    if (-not [string]::IsNullOrEmpty($tempRoot) -and (Test-Path -LiteralPath $tempRoot)) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

exit $exitCode
