# Fleet watchdog

Root-owned supervisor for the two Fleet services. It probes the admin and
daemon health endpoints and restarts one of them only after two consecutive
failures, with cooldown and rate limiting.

The watchdog is installed at the same time as the Fleet services and runs
outside the services it supervises:

| Platform | Scheduler | Runs as |
| --- | --- | --- |
| macOS | `dev.fleet.watchdog` LaunchDaemon, `StartInterval` 30s | root |
| Linux | `fleet-watchdog.timer`, every 30s | root (system unit) |
| Windows | `\Fleet\FleetWatchdog` scheduled task, every minute | `SYSTEM` |

Files in this directory and alongside it under `packaging/`:

| File | Purpose |
| --- | --- |
| `fleet-watchdog.sh` | POSIX `sh` watchdog for macOS and Linux |
| `fleet-watchdog.ps1` | Windows PowerShell watchdog |
| `packaging/systemd/fleet-watchdog.service.in` | oneshot systemd service |
| `packaging/systemd/fleet-watchdog.timer.in` | 30s systemd timer |
| `packaging/launchd/dev.fleet.watchdog.plist.in` | launchd LaunchDaemon |
| `packaging/windows/fleet-watchdog-task.xml.in` | SYSTEM scheduled task |

## Placeholders

Every value is substituted at install time. The watchdog never reads ports,
URLs, labels, or paths from user configuration at runtime.

| Placeholder | Files | Meaning and constraints |
| --- | --- | --- |
| `@ADMIN_HEALTH_URL@` | `.sh`, `.ps1` | Absolute `http://` or `https://` URL of the admin health endpoint. Example: `http://127.0.0.1:8787/api/v1/health`. Must not require authentication. |
| `@DAEMON_HEALTH_URL@` | `.sh`, `.ps1` | Absolute health URL of the local daemon. Example: `http://127.0.0.1:18080/health` (the daemon also serves `/api/v1/health`). Must not require authentication. |
| `@SERVICE_MANAGER@` | `.sh`, `.ps1` | Fixed command family. For `.sh`: `launchd` or `systemd`. For `.ps1`: `scheduledtask` or `service`. Any other value makes the watchdog exit without touching a service. |
| `@ADMIN_SERVICE_ID@` | `.sh`, `.ps1` | Admin service identifier: launchd label (e.g. `dev.fleet.admin`), systemd unit (e.g. `fleet-admin.service`), or a flat Windows scheduled-task/service name (e.g. `fleet-admin`). The shell watchdog permits `A-Z a-z 0-9 _ . @ -` (the `@` is for systemd template units); the PowerShell watchdog permits `A-Z a-z 0-9 _ . -`. |
| `@DAEMON_SERVICE_ID@` | `.sh`, `.ps1` | Daemon identifier with the same rules (e.g. `dev.fleet.daemon`, `fleet-daemon.service`, or `fleet-daemon`). |
| `@STATE_DIR@` | `.sh`, `.ps1` | Absolute, dedicated state directory. The watchdog creates it with mode `0700` when absent; the installer must ensure an existing directory is owned by root/SYSTEM and writable only by that account. |
| `@WATCHDOG_PATH@` | service, plist, task XML | Absolute installed path of the watchdog script. On Windows, the path to `fleet-watchdog.ps1`. |
| `@POWERSHELL_PATH@` | task XML | Absolute path to `powershell.exe`, normally `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` expanded at install time. Quote it if the expanded path contains whitespace. |
| `@TASK_START_BOUNDARY@` | task XML | Local ISO-8601 timestamp for the daily calendar trigger, e.g. `2026-11-17T00:00:00`. Use the install time (or a past time) so the one-minute repetition starts immediately. |

### How substitution works

The Fleet installer renders the values into the fixed assignment lines (and
the `ExecStart=`/`<string>` script path) and validates them before writing. An
installer should:

1. Read the template as text.
2. Replace every token above with a validated value.
3. Write the result atomically to its destination (`root:root/wheel` or a
   SYSTEM/Administrators ACL).
4. Install `fleet-watchdog.sh` with mode `0755` and its parent directory
   non-writable by ordinary users. Install the plist/units/task definition
   with an equally restrictive owner.

Values are inserted inside single-quoted assignments in the shell and
PowerShell files, and inside XML text in the templates. The installer must
therefore reject values containing a line break, a NUL byte, or a single
quote; XML values must also escape `&`, `<`, and `>`. Reject a `%` in paths
used by systemd (`%` introduces unit specifiers). Do not accept relative
paths or URLs with a scheme other than `http`/`https`.

## Behavior

On every run the watchdog checks the admin and daemon independently:

1. **Probe.** `curl -q --fail --silent --show-error --max-time 3 --noproxy '*'`
   (POSIX) or `Invoke-WebRequest -TimeoutSec 3` (Windows). Any HTTP error,
   timeout, connection failure, or a service that is not active counts as one
   failure.
2. **Failure threshold.** A restart is considered only on the second
   consecutive failure. A success clears the failure counter and starts a
   healthy streak.
3. **Restart.** Fixed commands only, selected by the manager value:
   * launchd: `launchctl kickstart -k system/<id>`
   * systemd: `systemctl restart <id>`
   * Windows scheduled task: `Stop-ScheduledTask` followed by
     `Start-ScheduledTask` (Windows PowerShell has no `Restart-ScheduledTask`
     cmdlet)
   * Windows service: `Restart-Service -Name <id>`

   The failure counter is reset after a restart, so the next restart again
   requires two fresh failures.
4. **Cooldown.** After the first restart the next restart is at least 30s
   away, after the second at least 1 minute, and after the third and later at
   least 5 minutes.
5. **Hard limit.** At most 5 restarts are allowed in a rolling 10-minute
   window. When the budget is exhausted the watchdog logs once and stops
   restarting until older attempts age out.
6. **Stability reset.** After 30 continuous healthy minutes the restart
   counter, cooldown, attempt history, and rate-limit marker are cleared.

The script exits `0` when the checks completed, `1` when it refused to act
(missing `curl`/`Invoke-WebRequest`, not root/SYSTEM, unusable state
directory, or state that cannot be persisted), and `2` when configuration is
invalid or a placeholder is still unsubstituted. In every refusal case no
service is restarted: the watchdog fails safe.

### State and logging

The POSIX watchdog stores one small file per service in `@STATE_DIR@`:
`admin.failures`, `admin.restarts`, `admin.attempts`, `admin.next_restart`,
`admin.healthy_since`, `admin.rate_limited`, and the `daemon.*` equivalents.
The Windows watchdog stores `admin.state.json` and `daemon.state.json`.
Values are validated as unsigned integers, so a corrupted state file cannot
become command input. The directory and its files are root/SYSTEM-owned.

Diagnostics go to stderr (captured by launchd/systemd or the task history) and
are appended to `@STATE_DIR@/watchdog.log` when that directory exists and is
writable. The log is append-only; rotate it with the platform's normal log
rotation if a deployment needs a size cap.

## Security model

* All paths, ports, URLs, labels, and manager choices are fixed by the
  installer. The watchdog never reads a user-controlled configuration file.
* There is no `eval`, no `source`/dot-sourcing, and no execution of a Fleet
  binary. The only restart commands are the fixed allow-list above.
* `PATH` is pinned to `/usr/bin:/bin:/usr/sbin:/sbin` before any command runs,
  so a user-writable directory such as `/usr/local/bin` or `~/.local/bin` can
  never supply a root-executed `curl`, `systemctl`, or `launchctl`.
* `curl -q` ignores `.curlrc`, and `--noproxy '*'` prevents proxy environment
  variables from redirecting a loopback probe. Windows clears the default
  web proxy before probing.
* Unsubstituted placeholders, invalid URLs, invalid identifiers, and unknown
  managers are rejected before the first probe.
* If the state directory cannot be written, the watchdog does not restart
  anything; without pacing, a watchdog can turn a crash loop into an outage.

## Platform notes

### macOS

Install the rendered plist in `/Library/LaunchDaemons/` (root:wheel, mode
`0644`) and the script in a root-owned directory such as
`/usr/local/libexec/fleet/`, then:

```sh
launchctl bootstrap system /Library/LaunchDaemons/dev.fleet.watchdog.plist
```

`launchctl kickstart` requires the target jobs to be loaded in the system
domain. The admin/daemon LaunchDaemons are installed by the service packaging,
not by this template.

### Linux

Install the rendered service and timer in `/etc/systemd/system/`
(root:root, mode `0644`) and the script as root-owned `0755`, then:

```sh
systemctl daemon-reload
systemctl enable --now fleet-watchdog.timer
```

The timer uses `OnActiveSec=30s` so the first probe runs shortly after the
timer starts, even if the service has never been activated before, and
`OnUnitActiveSec=30s` to pace later runs relative to each completed service
activation. `Persistent=true` is included as defensive configuration; it has
an effect only for calendar timers. The service intentionally has no `User=`,
and the timer is the unit to enable.

### Windows

Install the rendered task XML and script under a SYSTEM/Administrators-only
ACL, create `@STATE_DIR@` with the same ACL, and register the task from the XML
(using `Register-ScheduledTask -Xml` avoids `schtasks` encoding surprises):

```powershell
Register-ScheduledTask -TaskName 'FleetWatchdog' -TaskPath '\Fleet\' `
  -Xml (Get-Content -LiteralPath .\fleet-watchdog-task.xml -Raw) -Force
```

The task runs `powershell.exe` with `-NoProfile -NonInteractive` and repeats
every minute. The admin and daemon startup tasks are registered as
`fleet-admin` and `fleet-daemon` in the root task folder, which are the
identifiers to substitute for `@ADMIN_SERVICE_ID@` and `@DAEMON_SERVICE_ID@` in
`scheduledtask` mode. Because the cadence is one minute, the two-failure
threshold takes about two minutes, and the nominal 30s first cooldown is
effectively one minute. The task XML also has a boot trigger with a
30-second delay.

A scheduled task in `Running`, `Queued`, or `Ready` state is treated as an
active container; the health probe is authoritative for whether the supervised
process is alive. A missing or disabled task, and a Windows service that is not
`Running`, count as inactive.

## Assumptions

* Fleet's health endpoints are reachable without authentication on loopback,
  and `curl`/`Invoke-WebRequest` is present on the host.
* The admin and daemon are system-level services with stable identifiers
  supplied at install time; the watchdog does not discover them.
* `/bin/sh` is a POSIX shell (dash, ash, or macOS `sh`). The script is checked
  with `sh -n`; run `shellcheck` in the release pipeline when it is available.
* On non-standard Linux distributions where `systemctl` is not in
  `/usr/bin:/bin:/usr/sbin:/sbin`, the installer must adjust the pinned `PATH`
  before installing the script.
* Windows service/task identifiers are flat names (no folder path), which is
  what the validation regex permits.
