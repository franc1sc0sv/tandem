# Windows secure desktop (T-027)

Without help, the keyboard and mouse that come from the Mac stop at the PC's UAC prompts, the
lock screen, the sign-in screen and the Ctrl+Alt+Del screen: those live on the `Winlogon`
desktop, which a user process cannot reach, and UIPI also blocks injection into elevated
windows. Tandem solves it with a small SYSTEM service. Code: `crates/tandemd/src/secure/`.

## How it works

```
 session 0                         console session (1, 2, ...)
┌──────────────────────────┐      ┌──────────────────────────────────────────────┐
│ tandemd --service        │      │ tandemd --input-helper   (SYSTEM, UIAccess)  │
│ LocalSystem, auto start  │─────▶│ OpenInputDesktop + SetThreadDesktop          │
│ - follows WTS sessions   │spawn │ SendInput on Default / Winlogon / screensaver│
│ - restarts the helper    │◀─────│ pipe \\.\pipe\tandem-secure-input-<session>  │
│ - SendSAS (Ctrl+Alt+Del) │ "sas"│        ▲ SYSTEM + elevated admins only       │
└──────────────────────────┘stderr│        │ INPUT records, SAS requests         │
                                  │ tandemd (session agent, elevated task)       │
                                  └──────────────────────────────────────────────┘
```

- **Service** (`tandemd --service`, service name `TandemSecureDesktop`): runs as LocalSystem,
  starts with Windows and only supervises. It launches the helper in the session that owns the
  physical console (`WTSGetActiveConsoleSessionId`) with a copy of its own LocalSystem token
  (`DuplicateTokenEx`) moved to that session (`TokenSessionId`) with `TokenUIAccess = 1`, on
  `winsta0\default`, through `CreateProcessAsUser`. The helper inherits only its control
  pipe (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`) and runs in a job with `KILL_ON_JOB_CLOSE`, so it
  dies with the service however the service ends. WTS notifications
  (`SERVICE_CONTROL_SESSIONCHANGE`) and a
  1 s poll move the helper when the console changes session (fast user switching, sign-out,
  a remote session taking the console). A helper that dies comes back after 1, 2, 4 ... 30 s;
  one that lived a minute starts again from 1 s.
- **Helper** (`tandemd --input-helper`): before each injection it checks, at most every
  100 ms and right after any failed `SendInput`, which desktop has the input, and moves its
  thread there. A failed injection on the old desktop is retried once on the new one.
- **Session agent**: its Windows injector sends each batch of `INPUT` records to the helper
  while the pipe is up, and falls back to its own `SendInput` when it is not (it retries the
  pipe every 2 s). Going through the helper also on the normal desktop means elevated windows
  stop blocking the mouse. Ctrl+Alt+Delete (either Ctrl, either Alt, Delete or keypad Del) is
  not injected: the agent sends a SAS request, the helper writes `sas` on its stderr and the
  service calls `SendSAS(FALSE)`.
- **Protected screens module**: active while the agent reaches the helper; otherwise
  unavailable with the reason (service not installed, not running, agent not elevated, service
  from another version).

### Security

- The helper's token is the service's own, never another process's: looking up a process by
  name (`winlogon.exe`) would let a user start a renamed binary in their session and get
  *their* token UIAccess. Before setting `TokenUIAccess` the service checks that the token's
  user is LocalSystem and that it landed in the console session; otherwise nothing is launched
  (`supervisor::helper_token`, tested with a double and on the Windows CI).
- The pipe carries keystrokes and can click "Yes" on a UAC prompt. Its DACL allows only SYSTEM
  and Administrators, with a high-integrity label: an unelevated process (whose Administrators
  group is deny-only) cannot open it. So the agent must run elevated, which is what the
  scheduled task of T-104 does when Tandem is installed under Program Files.
- The helper creates the pipe with `FILE_FLAG_FIRST_PIPE_INSTANCE`; if someone squats the name
  first, the helper fails instead of sharing it. The agent opens the pipe at identification
  level (the server cannot impersonate it) and only talks to a server process that runs as
  LocalSystem.
- The service runs its own copy of `tandemd.exe` in `%ProgramFiles%\Tandem Service`, where only
  administrators write. The copy is only made from an executable that is administrators-only
  itself (the same ACL check as T-104), and the copy is checked again afterwards. Its logs go
  to `%ProgramFiles%\Tandem Service\logs` (not `%ProgramData%`, where a user could plant a link
  that SYSTEM would append to).
- The first request on the pipe must be a hello with the protocol version; the helper injects
  nothing for a client that does not send it.

## Limits

- The keyboard and mouse still come through the session agent, which runs in the user's
  session. The secure desktop works on everything that happens inside that session: UAC
  prompts, Ctrl+Alt+Del, the lock screen and its sign-in. Before anyone signs in after a boot,
  and in another account's session, there is no agent to receive the Mac's input; that needs
  the whole agent as a service and is not part of T-027.
- The pipe name, `\\.\pipe\tandem-secure-input-<session>`, is global and predictable. Any
  local process, even unelevated, can create it first (for example for a session that has no
  helper yet). The helper then fails to start and the agent falls back to its own `SendInput`:
  a denial of service of the secure desktop only, since the agent refuses a server that is not
  LocalSystem and the squatter never sees keystrokes.
- Capture on the secure desktop (the PC's own keyboard and mouse driving the Mac while a UAC
  prompt is up) is not covered: the agent's hooks do not see that desktop. The PC's input
  simply stays on the PC there.

## Installing

The app's setup has an **Install** button for the "Administrator service" row: on Windows it
runs, with one UAC prompt,

```
"C:\Program Files\Tandem\tandemd.exe" --install --with-service
```

which registers the elevated task (T-104) and installs the service. It requires Tandem itself
to be installed under Program Files (the per-user install in `%LOCALAPPDATA%` cannot elevate
safely; the row explains it). By hand, from an elevated terminal:

```
tandemd --install-service      # install or refresh the service only
tandemd --uninstall-service    # remove the service only
tandemd --install --with-service
tandemd --uninstall            # with administrator, also removes the service
```

`--install-service` does, in order: stop the service if it runs, create
`%ProgramFiles%\Tandem Service`, copy `tandemd.exe` there, check the copy's ACL, create (or
update) the service as LocalSystem with automatic start, set its description and recovery
actions (restart after 5 s, 10 s and 60 s; the failure count resets after a day; exits with an
error count as failures too), turn on the `SendSAS` policy if needed, and start it.

Every `tandemd --install` with administrator also refreshes an installed service with the new
binary, so an elevated update keeps it current. Without administrator (the normal per-user
update) the old service keeps running; if a release changes the pipe protocol, the module
says the service is from another version until it is reinstalled.

### Ctrl+Alt+Del policy

`SendSAS` from a service only works if the policy *Computer Configuration → Administrative
Templates → Windows Components → Windows Logon Options → Disable or enable software Secure
Attention Sequence* allows services. Its registry value is

```
HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System
  SoftwareSASGeneration (DWORD): 0 none, 1 services, 2 Ease of Access apps, 3 both
```

Installing turns on the services bit (0 or missing → 1, 2 → 3) and remembers the value it found
in `HKLM\SOFTWARE\Tandem\SecureDesktop\PreviousSoftwareSASGeneration`; uninstalling puts it
back. A domain Group Policy can override it: then ask the administrator to allow services. The
service log warns at start when the policy does not allow services.

## Authenticode signing

The release is unsigned today (DESIGN §13); signing is the user's decision, including which
certificate. What matters for the secure desktop:

- **What has to be signed:** `tandemd.exe` (it is the service and the helper, and the service
  runs a byte copy of it, so the signature carries over), `tandemw.exe`, the app's executable
  and the NSIS installer. All with the same certificate, SHA-256 digests and a timestamp, so
  the signature outlives the certificate. `tools/windows/installer/build-installer.ps1` already
  signs everything with `TANDEM_WINDOWS_CERT_THUMBPRINT` or `TANDEM_WINDOWS_SIGN_COMMAND` (see
  `docs/dev/install-windows.md`); the service needs nothing else.
- **Is it required for this design?** Not for the mechanism: the service sets `TokenUIAccess`
  directly on a SYSTEM token, and Windows only enforces "signed and in a secure location" for
  programs that ask for `uiAccess="true"` in their manifest (the policy *User Account Control:
  Only elevate UIAccess applications that are installed in secure locations*, and the signature
  check of the AppInfo service). Tandem's executables have no such manifest. If that ever
  changes, a valid Authenticode signature and an install under `%ProgramFiles%` become
  mandatory.
- **Why sign anyway:** a SYSTEM service that hands out a UIAccess token and injects input is
  exactly
  what antivirus heuristics look for. An unsigned build may be quarantined by Microsoft Defender
  or other products, is blocked by WDAC/AppLocker policies that only allow signed code, and the
  installer shows SmartScreen's "unknown publisher". An EV certificate (or Azure Trusted
  Signing) gets SmartScreen reputation right away; an OV certificate earns it over time.
- **Options:** an OV or EV code-signing certificate from a public CA (on a hardware token or
  HSM since 2023, so `TANDEM_WINDOWS_SIGN_COMMAND` with the vendor's tool is the usual path), or
  Azure Trusted Signing. A self-signed certificate only helps on machines that trust it
  explicitly.
- **Checking a build:**

  ```
  Get-AuthenticodeSignature "C:\Program Files\Tandem Service\tandemd.exe" | Format-List
  signtool verify /pa /v "C:\Program Files\Tandem Service\tandemd.exe"
  ```

The service install does not refuse unsigned binaries (the user decides about signing); it
relies on the administrators-only ACL of the source and of the copy.

## Manual test (needs the PC and UAC)

Prerequisites: Tandem installed under `C:\Program Files\Tandem` (run the installer as
administrator and choose that folder), so the agent runs from the elevated task; the Mac paired
and controlling the PC.

1. Install the service: setup → "Administrator service" → **Install**, accept UAC (or
   `tandemd --install --with-service` in an elevated terminal). Check with
   `sc.exe query TandemSecureDesktop` (`RUNNING`) and `sc.exe qfailure TandemSecureDesktop`
   (three `RESTART` actions), and that `C:\Program Files\Tandem Service\tandemd.exe` exists.
   In the app, the PC's "Protected screens" module turns active once the mouse crosses.
2. **UAC prompt:** from the Mac, open an elevated PowerShell on the PC (Start → type
   `powershell` → Ctrl+Shift+Enter). With the Mac's mouse and keyboard, move over the prompt,
   click **No**, open it again and click **Yes**. Also type into an elevated window.
3. **Ctrl+Alt+Del:** with the PC in focus, press Cmd+Option+Forward Delete on the Mac
   (Forward Delete is fn+Delete on a laptop keyboard; with the default Cmd↔Ctrl swap, Cmd is
   the PC's Ctrl; with the swap off, use Ctrl+Option). The Ctrl+Alt+Del screen appears; choose
   **Cancel** with the Mac's mouse.
4. **Lock screen:** lock the PC (Win+L from the Mac). Wake it and type the PIN or password
   with the Mac's keyboard.
5. **Fast user switching:** with a second Windows account, switch user from the Start menu.
   The service log shows the helper moving to the new session. The Mac's mouse does not drive
   the other account (no agent of yours runs there; see "Limits"). Switch back to your
   account: on its sign-in screen and after signing in, the Mac's mouse works again within a
   couple of seconds.
6. **Restart recovery:** kill the helper (`taskkill /F /FI "USERNAME eq SYSTEM" /IM tandemd.exe`
   from an elevated terminal kills the helper and the service): the SCM brings the service
   back within 5 s and it relaunches the helper.
7. **Uninstall:** `tandemd --uninstall-service` (elevated) or uninstall Tandem as administrator.
   `sc.exe query TandemSecureDesktop` says the service does not exist,
   `C:\Program Files\Tandem Service` is gone and `SoftwareSASGeneration` is back to its old
   value.

Logs: `C:\Program Files\Tandem Service\logs\service\tandemd.log` and `...\logs\helper\tandemd.log`.
