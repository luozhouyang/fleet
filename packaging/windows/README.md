# Windows packaging

Fleet's Windows install is **per-user first, with one elevation prompt**:

1. The user runs `install.ps1` from a normal (non-elevated) PowerShell prompt.
2. The script downloads the release archive and `SHA256SUMS`, verifies the
   archive, and extracts it to a temporary directory.
3. The script runs `fleet.exe install --from-dir <temp>` with the forwarded
   options and propagates its exit code.
4. `fleet install` writes the per-user files:
   - binaries: `%LOCALAPPDATA%\Fleet\bin`
   - web UI: `%LOCALAPPDATA%\Fleet\web`
   - configuration and data: `%USERPROFILE%\.fleet`
5. `fleet install` requests elevation **once** to register the Task Scheduler
   tasks from the templates in this directory. The admin and daemon tasks run
   with `LeastPrivilege` as the invoking user; the separate `SYSTEM` watchdog
   is the only elevated task.

The admin and the daemon are both `AtStartup` tasks with a stored credential.
A separate watchdog task (`fleet-watchdog-task.xml.in`) runs as `SYSTEM`; it is
owned by the watchdog workstream and is not part of the user-credential flow.

## Files

| File | Purpose |
| --- | --- |
| `apps/docs/public/install.ps1` | Public bootstrap served at `https://luozhouyang.github.io/fleet/install.ps1`. |
| `packaging/windows/fleet-admin-task.xml.in` | Template for the `fleet-admin` startup task. |
| `packaging/windows/fleet-daemon-task.xml.in` | Template for the `fleet-daemon` startup task. |
| `packaging/windows/fleet-watchdog-task.xml.in` | `SYSTEM` watchdog task, owned by the watchdog workstream. |
| `packaging/windows/README.md` | This contract. |

## install.ps1

Run it from a normal PowerShell window. It refuses to run if the process is
elevated, including with `-DryRun`.

```powershell
Invoke-WebRequest https://luozhouyang.github.io/fleet/install.ps1 -OutFile install.ps1 -UseBasicParsing
# review install.ps1, then:
.\install.ps1 -DryRun
.\install.ps1
```

| Parameter | Default | Behavior |
| --- | --- | --- |
| `-Version` | `latest` | `latest`, `v1.2.3`, or `1.2.3`. Selects the release base. |
| `-Prefix` | `%LOCALAPPDATA%\Fleet` | Install prefix. Expanded and forwarded to `fleet install`. |
| `-Port` | `8787` | Admin HTTP port. Forwarded to `fleet install`. |
| `-Yes` | off | Forwards `--yes`. |
| `-Force` | off | Forwards `--force`. |
| `-DryRun` | off | Prints the plan and exits without downloading or writing. |
| `-NoService` | off | Internal/test-only. Forwards `--no-service`. Not supported for production installs. |

Environment:

- `FLEET_BASE_URL` overrides the release base for mirrors and tests. It must be
  an absolute `http(s)` directory URL without embedded credentials, a query
  string, or a fragment, for example
  `https://mirror.example.com/fleet/releases/download/v0.1.0`. When it is set,
  it wins over `-Version` for URL resolution. Both the archive and
  `SHA256SUMS` are fetched from that base.

Architecture detection maps `AMD64`/`X64` to `x86_64` and
`ARM64`/`AARCH64` to `arm64`. At the time of writing, the release workflow
publishes only `fleet-windows-x86_64.zip`; an arm64 install fails with a clear
"not found" error until that asset is published.

### Forwarded CLI contract

The bootstrap always runs exactly this shape:

```text
fleet.exe install --from-dir <extract-dir> --prefix <prefix> --port <port> [--yes] [--force] [--no-service]
```

- `--prefix` and `--port` are always forwarded with the resolved values.
- `--yes`, `--force`, and the hidden `--no-service` are forwarded only when
  the corresponding switch is set.
- `-DryRun` never invokes `fleet`.
- `fleet install` must finish reading and copying from `--from-dir` before it
  returns, including across its synchronous elevation prompt. The bootstrap
  removes the temp directory as soon as the child exits.
- The child's exit code is the script's exit code. A missing or failing
  `fleet install` command is therefore visible to callers and CI.

The `fleet install` command is part of the parallel Rust implementation. This
bootstrap depends on that command accepting the flags above. If the command is
absent or rejects an option, `fleet` prints a usage error and the bootstrap
exits non-zero.

### Checksum policy

- `SHA256SUMS` is mandatory. A missing sums file, a missing entry for the
  selected asset, or a hash mismatch is fatal; there is no `-SkipChecksum`.
- Verification uses `Get-FileHash -Algorithm SHA256` and compares the lowercase
  hex digest with the `SHA256SUMS` entry for the exact asset name.
- `SHA256SUMS` is fetched from the same base as the archive. This detects
  corruption and truncated downloads; it is **not** a signature. A compromised
  release origin or mirror controls both files.

### Elevation guard

`install.ps1` checks `WindowsPrincipal.IsInRole(Administrator)` and exits
non-zero with an explanation if it is elevated. Do not work around this by
running the whole installer as administrator: the per-user files would be
created under the administrator's profile and ACLs, and the normal user's
tasks would point at the wrong paths. Elevation belongs only to the short
service-registration step inside `fleet install`.

## Task Scheduler templates

Both user-task templates use the Task Scheduler XML namespace
`http://schemas.microsoft.com/windows/2004/02/mit/task` and contain:

- `<BootTrigger>` (the XML form of "At startup").
- `<Principal id="Author">` with `@USER@`, `LogonType=Password`, and
  `RunLevel=LeastPrivilege`.
- `MultipleInstancesPolicy=IgnoreNew`, battery start/stop allowed,
  `AllowHardTerminate=true`, `StartWhenAvailable=true`,
  `ExecutionTimeLimit=PT0S`, and `RestartOnFailure` with `PT1M`/`3`.
- One `<Exec>` action built from `@FLEET_BIN@`, `@ARGS@`, and
  `@WORKING_DIR@`.

### Placeholders

Every placeholder is an entire element text value. Substitute only after
validating and escaping the value. The renderer must not insert a value that
contains unescaped markup.

| Placeholder | Required value | Rules |
| --- | --- | --- |
| `@USER@` | Account that owns the installation, in `DOMAIN\user` or `MACHINE\user` form. Example: `CONTOSO\alice`. | Must be the same account whose `%USERPROFILE%` and `%LOCALAPPDATA%` `fleet install` used. XML-escape. Reject newlines and control characters. The account password is supplied at registration, never in this element. |
| `@FLEET_BIN@` | Absolute path to the role binary. Admin: `C:\Users\alice\AppData\Local\Fleet\bin\fleet-admin.exe`. Daemon: `...\fleet-daemon.exe`. | Expand environment variables before substitution; Task Scheduler should not be relied on to expand `%VAR%`. XML-escape. If the path contains whitespace, wrap it in quotes (rendered as `&quot;`) because `<Command>` is parsed as a command line. Keep the path within the schema's 260-character limit. |
| `@ARGS@` | Complete argument string for the role binary. Admin: `--config "C:\Users\alice\.fleet\admin\config.toml" serve --windows-task`. Daemon: `--config "C:\Users\alice\.fleet\daemon\config.toml" serve --windows-task`. | XML-escape `&`, `<`, `>`, `"`, and `'`. Keep quotes around paths with spaces. Never include tokens, passwords, or other secrets. |
| `@WORKING_DIR@` | Absolute role state directory. Admin: `C:\Users\alice\.fleet\admin`. Daemon: `C:\Users\alice\.fleet\daemon`. | Expand environment variables and XML-escape. This is a directory path, not a command line, so do not add shell quoting. Keep it within the 260-character limit. |

The examples use an unpacked user profile for readability. `fleet install`
should render expanded absolute paths from its resolved role paths; do not
leave `%LOCALAPPDATA%` or `%USERPROFILE%` in the XML.

Task names are part of the operational contract: `fleet-admin` and
`fleet-daemon` in the root task folder. The watchdog uses
`\Fleet\FleetWatchdog` instead, with a `SYSTEM` principal.

### Watchdog template

`fleet-watchdog-task.xml.in` is the separate `SYSTEM` watchdog. It does not
use stored user credentials: its principal is `S-1-5-18`
(`NT AUTHORITY\SYSTEM`) with `LogonType=ServiceAccount` and
`RunLevel=HighestAvailable`, and the task is hidden. It has a boot trigger with
a 30-second delay plus a daily calendar trigger that repeats every minute, and
it runs PowerShell with the installed watchdog script.

| Placeholder | Required value | Rules |
| --- | --- | --- |
| `@TASK_START_BOUNDARY@` | Local ISO-8601 timestamp for the calendar trigger, e.g. `2026-11-17T00:00:00`. | Use the install time or a past time so the one-minute repetition starts immediately. XML-escape even though the timestamp normally needs no escaping. |
| `@POWERSHELL_PATH@` | Absolute path to `powershell.exe`, normally `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`. | Expand environment variables at install time, verify the file, XML-escape, and quote if the path contains whitespace. |
| `@WATCHDOG_PATH@` | Absolute installed path of `fleet-watchdog.ps1`. | The template already quotes the argument as `&quot;...&quot;`; substitute the raw path and XML-escape it. Install the script and task XML with a SYSTEM/Administrators-only ACL because the task runs elevated. |

The watchdog's probe URLs, service identifiers, state directory, restart
policy, and security model are documented in `packaging/watchdog/README.md`.
Register it from the same elevated step with
`Register-ScheduledTask -Xml ...`; it needs no password.

### Encoding

The two user-task `.in` files are UTF-8 so they can be reviewed, diffed, and
validated as standalone XML. Their declarations intentionally omit an
`encoding` attribute.

The existing CLI's Task Scheduler backend writes task XML as UTF-16LE with a
BOM before passing it to `schtasks /XML`. The renderer should do the same:

1. Read the template as UTF-8.
2. Substitute and XML-escape the four placeholders.
3. Write the result as UTF-16LE with a BOM (adding
   `encoding="UTF-16"` to the declaration is optional because the BOM
   identifies the encoding).
4. Register the file from an elevated process.

Do not register a file whose bytes are UTF-8 while its declaration claims
UTF-16; XML parsers reject that mismatch.

### Registration and stored credentials

The XML never contains a password. `LogonType=Password` tells Task Scheduler
to authenticate the `@USER@` account at registration and store the credential
for startup use. The elevated helper must obtain that credential securely and
must not write it to the template, the rendered XML, logs, or a command line.

Preferred interactive registration lets `schtasks` prompt for the run-as
password:

```powershell
schtasks.exe /Create /TN "fleet-admin" /XML $adminXml /F
schtasks.exe /Create /TN "fleet-daemon" /XML $daemonXml /F
schtasks.exe /Run /TN "fleet-admin"
schtasks.exe /Run /TN "fleet-daemon"
```

`/RP <password>` also works and is required for a non-interactive helper, but
it exposes the password in the process command line and possibly command
history. If the helper must be non-interactive, prefer
`Register-ScheduledTask -Xml <string> -User <user> -Password <password>`,
where the password comes from `Get-Credential` or a `SecureString` and is
never logged. If `/RU` is used with `schtasks`, it must match `@USER@`.

Task Scheduler stores the credential as a task secret. It is not in the XML
and should not be recoverable through ordinary task export. Re-registering the
task replaces it.

### Credential and startup risks

- A password change, expiry, or account lockout breaks the stored credential.
  The tasks then fail to start (typically `0x8007052E`, "the user name or
  password is incorrect") even though the XML is valid. Re-run the install or
  repair flow to update the stored password.
- Blank-password accounts, Windows Hello-only/passwordless accounts,
  Microsoft-account aliases, and some federated/Azure AD identities may not
  support `LogonType=Password` startup tasks. Test the exact account format on
  the target machine; prefer `DOMAIN\user` for domain and local accounts.
- Domain policy can block the stored-credential model even when registration
  succeeds: the account may need the "Log on as a batch job"
  (`SeBatchLogonRight`) right, and policy may forbid storing passwords for
  scheduled tasks. Verify with `schtasks /Query /TN fleet-admin /V` and the
  Task Scheduler operational log; if the task is created but never starts,
  check these rights before changing the XML.
- `AtStartup` with a stored password starts the task at boot, before that user
  signs in, and without loading a full interactive session. The user profile
  registry hive and file paths are available, but mapped drives, user-level
  network connections, and some desktop integrations may not be. The servers
  must tolerate a slow network: `StartWhenAvailable` and the three one-minute
  restarts help, and the `SYSTEM` watchdog provides longer-term recovery.
- Both tasks use `LeastPrivilege`. The account must own the repositories,
  agent CLIs, and configuration it needs; do not grant it administrator rights
  to compensate.
- Both binaries accept `--windows-task`, which switches tracing to their
  rotating file under `<state-dir>\logs\`. A task's stdout is not captured by
  Task Scheduler, so include the flag in `@ARGS@`; otherwise logs are
  effectively discarded when running as a task.
- The Task Scheduler schema limits `<Command>` and `<WorkingDirectory>` to 260
  characters. Deeply nested user profiles or custom prefixes can exceed this.
- Task Scheduler XML injection is possible if placeholders are substituted
  without escaping. A username can contain `&`; a path can contain `&` or
  quotes. Escape every value and keep placeholders out of XML comments, since
  substituted arguments contain `--` and XML comments cannot contain `--`.
- The bootstrap is an unsigned script and runs `fleet.exe` from `%TEMP%`.
  Execution policy, SmartScreen, or AppLocker/WDAC policy may block either
  step. Review the script, then unblock it per file or use a per-process
  `-ExecutionPolicy Bypass`; allow-list the temporary extraction path if the
  endpoint security policy forbids running binaries from `%TEMP%`. Do not
  weaken machine-wide policy or UAC just to install Fleet.
- `install.ps1` never prints tokens, rejects credentials and query strings in
  `FLEET_BASE_URL`, and sanitizes URLs in its own output. It streams
  `fleet install` stdout/stderr without capturing it so prompts and errors stay
  visible; the Rust installer must not print the admin token, pairing secret,
  service password, or other secrets. Treat the `FLEET_BASE_URL` path as public
  because `-DryRun` prints the resolved download URLs.

## Validation

On Windows, these checks, run from the repository root, do not download or
install anything:

```powershell
# Syntax-only parse (works in Windows PowerShell 5.1 and PowerShell 7)
$tokens = $null
$errors = $null
[System.Management.Automation.Language.Parser]::ParseFile(
  (Resolve-Path .\apps\docs\public\install.ps1).Path, [ref]$tokens, [ref]$errors) | Out-Null
if ($errors.Count -gt 0) { $errors; exit 1 }

# Dry run: resolves version, architecture, prefix, URLs, and command
.\apps\docs\public\install.ps1 -DryRun

# Well-formed XML
[xml](Get-Content -Raw .\packaging\windows\fleet-admin-task.xml.in) | Out-Null
[xml](Get-Content -Raw .\packaging\windows\fleet-daemon-task.xml.in) | Out-Null
[xml](Get-Content -Raw .\packaging\windows\fleet-watchdog-task.xml.in) | Out-Null
```

On other platforms, parse the XML with any conforming parser and review the
PowerShell statically; `install.ps1` deliberately exits immediately when it is
not running on Windows.
