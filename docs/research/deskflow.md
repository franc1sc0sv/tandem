# Deskflow — how to control it from the outside

Researched on `deskflow/deskflow` master (`c46781f`, 2026-10-14, after v1.27.0).
What is marked **(unverified)** was not tested on a real machine.

## Processes

- Binaries: `deskflow` (GUI), `deskflow-core` (server and client) and, on Windows only, `deskflow-daemon` (service that runs as SYSTEM).
- Startup: `deskflow-core server|client --settings <file.conf> [--new-instance]`. It is the only CLI there is; everything else goes in the INI.
- On macOS the core is at `Deskflow.app/Contents/MacOS/deskflow-core`.

## Configuration (INI `Deskflow.conf`)

- `[core]`: `coreMode` (1 = client, 2 = server), `interface`, `port` (24800), `computerName`, `preventSleep`.
- `[client]`: `remoteHost`, which accepts a **comma-separated list** (round-robin, since 1.26). Our fallback list goes there: USB4 → Ethernet → Wi-Fi.
- `[security]`: `tlsEnabled`, `checkPeerFingerprints`, `certificate`.
- Since 1.27, each computer is declared as a `[computer_<name>]` group.
- Server layout: `<settingsPath>/deskflow-server.conf`, with `links` and `options` sections. The `screens` section is now ignored.

## Live changes

- macOS: `SIGHUP` reloads the layout without restarting.
- Windows: the core has to be restarted.
- Changing role = rewriting the INI and restarting (≈1–3 s).
- Drop detection: keep-alive every 3 s, peer dead after 3 failures (≈9 s). Our agent can detect it sooner and force the switch.

## IPC (internal protocol, no stability guarantee)

- Qt local socket `deskflow-core`; on Windows also `deskflow-daemon`. Text messages `cmd=args\n`.
- Events: `connectionState`, `connectedClients`, `peerFingerprint`, `connectionRefused`, `secureSocket`.
- Core commands: only `stop`. Daemon commands: `configFile=`, `start`, `stop`, `logLevel=`.
- → We pin the version and isolate this in an adapter.

## TLS

- `tls/` folder with `deskflow.pem`, `trusted-servers` and `trusted-clients`; line format `v2:sha256:<hex>`. It is re-read on every handshake.
- Paths: macOS `~/Library/Deskflow/tls/`, Windows `C:\ProgramData\Deskflow\tls\`.
- **It can be pre-provisioned** from our pairing, so the user never sees the fingerprint dialog.

## Other

- File drag: **removed** (#8459). File transfer: it will not be done (#8104, #9648). → Tandem does it through the data plane.
- Clipboard: text, HTML and BMP image; it does not copy files.
- No mDNS (#8965 closed). → Discovery is done by our agent.
- macOS: requires Accessibility permission. Perhaps our agent needs it too, as the process that launches it **(unverified)**.
- Windows: to work on the sign-in screen and on UAC you have to go through the daemon.
- GPL-2.0 license with OpenSSL exception. We install it separately (`brew tap deskflow/tap`, `winget install Deskflow.Deskflow`), we do not bundle it.
