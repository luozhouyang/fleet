#requires -Version 5.1
<#
 Fleet watchdog (Windows).

 Installed so that only SYSTEM (or Administrators) can modify it, and run by a
 SYSTEM scheduled task every minute. It probes the admin and daemon health
 endpoints and restarts a service/task only after two consecutive failures,
 with cooldown and rate limits.

 SECURITY INVARIANTS
   * Every placeholder below is substituted at install time by the Fleet
     installer. Nothing is read from a user-controlled file at runtime.
   * The script runs with -NoProfile -NonInteractive (see the task XML), uses
     no Invoke-Expression, and calls no Fleet binary.
   * Probe and restart cmdlets are fixed; the only variable inputs are the
     install-time URLs and identifiers.
   * State is kept under @STATE_DIR@, which the installer creates with an ACL
     that grants access only to SYSTEM and Administrators.

 Exit status: 0 when both checks completed, 1 when the watchdog refused to act
 (missing capability, unusable state directory, persistence failure), and 2
 for invalid or unsubstituted configuration at startup.
#>

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$AdminHealthUrl = '@ADMIN_HEALTH_URL@'
$DaemonHealthUrl = '@DAEMON_HEALTH_URL@'
$ServiceManager = '@SERVICE_MANAGER@'
$AdminServiceId = '@ADMIN_SERVICE_ID@'
$DaemonServiceId = '@DAEMON_SERVICE_ID@'
$StateDir = '@STATE_DIR@'

$ProbeTimeoutSeconds = 3
$FailureThreshold = 2
$AttemptWindowSeconds = 600
$MaxAttempts = 5
$HealthyResetSeconds = 1800
$CooldownFirstSeconds = 30
$CooldownSecondSeconds = 60
$CooldownMaxSeconds = 300

$LogFile = Join-Path $StateDir 'watchdog.log'

function Write-WatchdogLog {
    param([Parameter(Mandatory = $true)][string]$Message)

    $timestamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $line = '{0} fleet-watchdog: {1}' -f $timestamp, $Message
    try {
        [Console]::Error.WriteLine($line)
    } catch {
        # No console attached (non-interactive scheduled task); the log file
        # remains the durable record.
    }
    try {
        Add-Content -LiteralPath $LogFile -Value $line -Encoding UTF8 -ErrorAction Stop
    } catch {
        # The caller has not created the state directory yet, or it is read
        # only. stderr above is still the primary destination.
    }
}

function Stop-WatchdogConfiguration {
    param([string]$Message)
    Write-WatchdogLog $Message
    exit 2
}

# ---------------------------------------------------------------------------
# Startup validation. Unsubstituted placeholders fail closed.
# ---------------------------------------------------------------------------

if ($ServiceManager -ne 'scheduledtask' -and $ServiceManager -ne 'service') {
    Stop-WatchdogConfiguration ("invalid service manager '{0}' (expected scheduledtask or service)" -f $ServiceManager)
}

foreach ($url in @($AdminHealthUrl, $DaemonHealthUrl)) {
    $parsed = $null
    if (-not [System.Uri]::TryParse($url, [ref]$parsed) -or $null -eq $parsed) {
        Stop-WatchdogConfiguration ("invalid health URL '{0}'" -f $url)
    }
    if ($parsed.Scheme -ne 'http' -and $parsed.Scheme -ne 'https') {
        Stop-WatchdogConfiguration ("invalid health URL '{0}' (expected http or https)" -f $url)
    }
}

foreach ($serviceId in @($AdminServiceId, $DaemonServiceId)) {
    if ($serviceId -notmatch '\A[A-Za-z0-9_.-]+\z') {
        Stop-WatchdogConfiguration ("invalid service identifier '{0}'" -f $serviceId)
    }
}

if (-not [System.IO.Path]::IsPathRooted($StateDir)) {
    Stop-WatchdogConfiguration 'state directory must be an absolute path'
}

if (-not (Get-Command -Name Invoke-WebRequest -CommandType Cmdlet -ErrorAction SilentlyContinue)) {
    Write-WatchdogLog 'Invoke-WebRequest is unavailable; refusing to manage services'
    exit 1
}

if ($ServiceManager -eq 'service') {
    if (-not (Get-Command -Name Get-Service -CommandType Cmdlet -ErrorAction SilentlyContinue) -or
        -not (Get-Command -Name Restart-Service -CommandType Cmdlet -ErrorAction SilentlyContinue)) {
        Write-WatchdogLog 'service cmdlets are unavailable; refusing to manage services'
        exit 1
    }
} else {
    if (-not (Get-Command -Name Get-ScheduledTask -CommandType Cmdlet -ErrorAction SilentlyContinue) -or
        -not (Get-Command -Name Start-ScheduledTask -CommandType Cmdlet -ErrorAction SilentlyContinue) -or
        -not (Get-Command -Name Stop-ScheduledTask -CommandType Cmdlet -ErrorAction SilentlyContinue)) {
        Write-WatchdogLog 'scheduled task cmdlets are unavailable; refusing to manage services'
        exit 1
    }
}

# Health endpoints are loopback-only; never route the probe via a system proxy.
try {
    [System.Net.WebRequest]::DefaultWebProxy = $null
} catch {
    # Leave the default proxy in place if the runtime refuses the assignment.
}

if (-not (Test-Path -LiteralPath $StateDir -PathType Container)) {
    try {
        New-Item -ItemType Directory -Path $StateDir -Force -ErrorAction Stop | Out-Null
    } catch {
        Write-WatchdogLog ("cannot create state directory '{0}'; refusing to manage services" -f $StateDir)
        exit 1
    }
}

try {
    $writeProbe = Join-Path $StateDir ('.watchdog-write-test.{0}' -f $PID)
    Set-Content -LiteralPath $writeProbe -Value 'ok' -Encoding ASCII -ErrorAction Stop
    Remove-Item -LiteralPath $writeProbe -Force -ErrorAction Stop
} catch {
    Write-WatchdogLog ("state directory '{0}' is not writable; refusing to manage services" -f $StateDir)
    exit 1
}

# ---------------------------------------------------------------------------
# State helpers.
# ---------------------------------------------------------------------------

function New-WatchdogState {
    return [pscustomobject]@{
        failures     = [long]0
        restarts     = [long]0
        attempts     = @()
        nextRestart  = [long]0
        healthySince = [long]0
        rateLimited  = $false
    }
}

function Get-WatchdogState {
    param([string]$Label)

    $state = New-WatchdogState
    $path = Join-Path $StateDir ('{0}.state.json' -f $Label)
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        return $state
    }

    try {
        $loaded = Get-Content -LiteralPath $path -Raw -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop

        foreach ($name in @('failures', 'restarts', 'nextRestart', 'healthySince')) {
            $value = $loaded.$name
            if (($value -is [int]) -or ($value -is [long]) -or ($value -is [double])) {
                if ($value -ge 0) {
                    $state.$name = [long]$value
                }
            }
        }

        $attempts = @()
        foreach ($attempt in @($loaded.attempts)) {
            if ($null -eq $attempt) {
                continue
            }
            if (($attempt -is [int]) -or ($attempt -is [long]) -or ($attempt -is [double])) {
                if ($attempt -gt 0) {
                    $attempts += [long]$attempt
                }
            }
        }
        $state.attempts = @($attempts)
        if ($loaded.rateLimited -is [bool]) {
            $state.rateLimited = $loaded.rateLimited
        }
    } catch {
        Write-WatchdogLog ("{0} state file is unreadable; resetting counters" -f $Label)
    }

    return $state
}

function Save-WatchdogState {
    param([string]$Label, $State)

    $path = Join-Path $StateDir ('{0}.state.json' -f $Label)
    $tmp = '{0}.tmp' -f $path
    $json = $State | ConvertTo-Json -Compress
    Set-Content -LiteralPath $tmp -Value $json -Encoding UTF8 -ErrorAction Stop
    Move-Item -LiteralPath $tmp -Destination $path -Force -ErrorAction Stop
}

# ---------------------------------------------------------------------------
# Fixed probes and restart commands.
# ---------------------------------------------------------------------------

function Test-FleetHealth {
    param([string]$Url)

    try {
        Invoke-WebRequest -UseBasicParsing -Uri $Url -Method Get -TimeoutSec $ProbeTimeoutSeconds -ErrorAction Stop | Out-Null
        return $true
    } catch {
        return $false
    }
}

function Test-FleetServiceActive {
    param([string]$ServiceId)

    if ($ServiceManager -eq 'service') {
        try {
            $service = Get-Service -Name $ServiceId -ErrorAction Stop
            return ($service.Status -eq 'Running')
        } catch {
            return $false
        }
    }

    # A scheduled task is a valid supervisor container in Running, Queued, or
    # Ready state. Ready only means the task is registered and not currently
    # executing; the health probe is what detects a dead supervised process.
    try {
        $task = Get-ScheduledTask -TaskName $ServiceId -ErrorAction Stop
        return ($task.State -ne 'Disabled' -and $task.State -ne 'Unknown')
    } catch {
        return $false
    }
}

function Restart-FleetService {
    param([string]$ServiceId)

    if ($ServiceManager -eq 'service') {
        Restart-Service -Name $ServiceId -ErrorAction Stop
        return
    }

    # Windows PowerShell has no Restart-ScheduledTask cmdlet. Stopping and
    # starting the fixed task is the equivalent, and both calls are fixed.
    Stop-ScheduledTask -TaskName $ServiceId -ErrorAction SilentlyContinue
    Start-ScheduledTask -TaskName $ServiceId -ErrorAction Stop
}

# ---------------------------------------------------------------------------
# Per-service check.
# ---------------------------------------------------------------------------

function Invoke-FleetWatchdogCheck {
    param(
        [string]$Label,
        [string]$Url,
        [string]$ServiceId
    )

    $state = Get-WatchdogState -Label $Label
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()

    $probeOk = Test-FleetHealth -Url $Url
    $activeOk = Test-FleetServiceActive -ServiceId $ServiceId

    if ($probeOk -and $activeOk) {
        $dirty = $false

        if ($state.failures -gt 0) {
            Write-WatchdogLog ("{0} is healthy again" -f $Label)
            $state.failures = [long]0
            $dirty = $true
        }
        if ($state.rateLimited) {
            $state.rateLimited = $false
            $dirty = $true
        }

        if ($state.healthySince -eq 0) {
            $state.healthySince = [long]$now
            $dirty = $true
        } elseif (($now - $state.healthySince) -ge $HealthyResetSeconds) {
            if ($state.restarts -gt 0) {
                Write-WatchdogLog ("{0} has been healthy for 30 minutes; resetting watchdog counters" -f $Label)
            }
            $state.restarts = [long]0
            $state.attempts = @()
            $state.nextRestart = [long]0
            $state.healthySince = [long]$now
            $dirty = $true
        }

        if ($dirty) {
            try {
                Save-WatchdogState -Label $Label -State $state
            } catch {
                Write-WatchdogLog ("cannot persist state for {0}" -f $Label)
            }
        }
        return $true
    }

    # Unhealthy or inactive. A broken health streak starts over.
    $state.healthySince = [long]0

    if (-not $probeOk -and -not $activeOk) {
        $reason = 'health probe failed and service is inactive'
    } elseif (-not $probeOk) {
        $reason = 'health probe failed'
    } else {
        $reason = 'service is inactive'
    }

    $state.failures = [long]($state.failures + 1)
    try {
        Save-WatchdogState -Label $Label -State $state
    } catch {
        Write-WatchdogLog ("cannot persist failure counter for {0}; not restarting" -f $Label)
        return $false
    }

    if ($state.failures -eq 1) {
        Write-WatchdogLog ("{0}: {1} (failure 1 of {2} before restart)" -f $Label, $reason, $FailureThreshold)
        return $true
    }

    $cutoff = $now - $AttemptWindowSeconds
    $state.attempts = @($state.attempts | Where-Object { $_ -gt $cutoff })

    if (@($state.attempts).Count -ge $MaxAttempts) {
        if (-not $state.rateLimited) {
            Write-WatchdogLog ("{0}: {1} restarts within 10 minutes; suspending restarts" -f $Label, $MaxAttempts)
            $state.rateLimited = $true
        }
        try {
            Save-WatchdogState -Label $Label -State $state
        } catch {
            Write-WatchdogLog ("cannot persist restart history for {0}" -f $Label)
        }
        return $true
    }

    if ($now -lt $state.nextRestart) {
        if ($state.failures -eq $FailureThreshold) {
            Write-WatchdogLog ("{0}: restart cooldown active until {1}; waiting" -f $Label, $state.nextRestart)
        }
        try {
            Save-WatchdogState -Label $Label -State $state
        } catch {
            Write-WatchdogLog ("cannot persist restart history for {0}" -f $Label)
        }
        return $true
    }

    $state.restarts = [long]($state.restarts + 1)
    if ($state.restarts -le 1) {
        $cooldown = $CooldownFirstSeconds
    } elseif ($state.restarts -eq 2) {
        $cooldown = $CooldownSecondSeconds
    } else {
        $cooldown = $CooldownMaxSeconds
    }
    $state.attempts = @($state.attempts) + [long]$now
    $state.nextRestart = [long]($now + $cooldown)
    $state.rateLimited = $false

    # Persist the decision before acting. If we cannot pace ourselves, we must
    # not restart.
    try {
        Save-WatchdogState -Label $Label -State $state
    } catch {
        Write-WatchdogLog ("cannot persist restart decision for {0}; not restarting" -f $Label)
        return $false
    }

    Write-WatchdogLog ("{0}: {1}; restarting via {2} (attempt {3}, cooldown {4}s)" -f `
            $Label, $reason, $ServiceManager, $state.restarts, $cooldown)
    try {
        Restart-FleetService -ServiceId $ServiceId
        Write-WatchdogLog ("{0} restart command succeeded" -f $Label)
    } catch {
        Write-WatchdogLog ("{0} restart command failed: {1}" -f $Label, $_.Exception.Message)
    }

    # Require two fresh consecutive failures before the next restart.
    $state.failures = [long]0
    try {
        Save-WatchdogState -Label $Label -State $state
    } catch {
        Write-WatchdogLog ("cannot reset failure counter for {0}" -f $Label)
    }
    return $true
}

$exitCode = 0
if (-not (Invoke-FleetWatchdogCheck -Label 'admin' -Url $AdminHealthUrl -ServiceId $AdminServiceId)) {
    $exitCode = 1
}
if (-not (Invoke-FleetWatchdogCheck -Label 'daemon' -Url $DaemonHealthUrl -ServiceId $DaemonServiceId)) {
    $exitCode = 1
}

exit $exitCode
