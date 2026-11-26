# Tandem — Design document

> Status: **v0.3** (2026-11-16). Source of truth for the project together with [DECISIONS.md](DECISIONS.md) (which wins if they clash).
> Visual reference: [`../mockups/v03/`](../mockups/v03/) (see [`../mockups/README.md`](../mockups/README.md)).
>
> **Change of course v0.3:** two peer machines with no fixed system per side; tunnels so Claude can use the other machine; monitors through a single standard (our own firmware) with a shared Rust crate; scenes instead of modes; all configuration from a new UI with a sidebar.

## 1. What it is

Tandem turns **two computers** (today a MacBook Pro and a Windows PC, but either one can be macOS or Windows) into a single desk:

- **Shared keyboard and mouse** with our own engine, with no owner: you use them from either machine and Tandem follows the one in use; the cursor crosses through the edge the arrangement says.
- **Shared clipboard** and **drag files** from one screen to the other.
- **Tunnels:** Claude (through MCP or the CLI) opens a tunnel to the other machine and inside it runs processes, copies files, opens ports and works in a git workspace that comes back as a branch.
- **Monitors:** every monitor that supports the standard (our own firmware) is controlled from either machine; **scenes** save what each monitor shows and the arrangement.
- **Power:** power the other machine on and off.
- Our own encrypted **link** that uses the best available path (USB-C, Ethernet, Wi-Fi).

The UI is **beautiful, visual and animated**: understand the state at a glance, read little, icons everywhere.

## 2. Hardware (this user's; the code does not assume it)

| | Machine 1 | Machine 2 |
|---|---|---|
| Computer | MacBook Pro (Apple Silicon, macOS 27) | Gigabyte X870 GAMING X WIFI7 · Ryzen 7 9800X3D · RTX 5080 · Windows 11 25H2 |
| Network | Wi-Fi; 3× Thunderbolt/USB4 | Wi-Fi 7 · Realtek 2.5GbE · USB4 (ASMedia ASM4242, firmware 0.1a, BIOS F3) |
| Video | HDMI → monitors | DisplayPort → monitors |

| Monitor | Model | Status |
|---|---|---|
| 34" | Samsung Odyssey G5 **LC34G55T** (MStar MST9U) | Own firmware v3.1 (protocol 2): reports its real input and gives a verdict on every change. **Supported.** |
| 32" | Samsung Odyssey G5 "FALCON" (Novatek NT68500) | **Out of service** since 2026-11-16 (damaged firmware, recoverable over SPI). When it returns with factory firmware: **not supported** until its patch. |

- **USB-C cable (spike S5, 2026-11-16):** with the 10 Gbps cable both ends negotiate over PD and the PC advertises USB4, but the USB4 link does not train; with a USB 2.0 cable it is impossible. A 40 Gbps Thunderbolt 4 / USB4 cable will be tried; the BIOS is not updated (user's decision). Details in [research/usb4-link.md](research/usb4-link.md).
- Test access: the PC from WSL (`powershell.exe`); the Mac through `ssh mac-runner` (Wi-Fi). See [dev/hardware.md](dev/hardware.md).

## 3. Principles

1. **One single app, all in Rust** (agent) + Tauri 2 with Svelte 5 (UI).
2. **Input comes first.** Nothing (tunnels, files, UI, monitors) may slow the capture → link → injection path. Every change on that path is measured (§7.1).
3. **Two equal sides.** `Machine::{A, B}`, ordered by public key. No module asks "is it the Mac?": it asks about **capabilities** (§4.3).
4. **Every action with an effect on the other machine is a transaction**: steps, timeouts and rollback (power, scenes, input changes).
5. **No guessing.** A monitor is controlled only if it tells the truth (standard firmware). An unknown state is shown as unknown.
6. **Configuration from the UI**, synced; the file is storage.
7. **Secure by default.** One-time pairing with a code, everything encrypted and mutually authenticated.
8. **Nothing meaningless in the UI.** Every element has a purpose and really works; if something is unavailable, it says why. No filler text or superfluous explanations.

## 4. Architecture

```
┌──────────── Machine A ─────────────┐        ┌──────────── Machine B ─────────────┐
│ Tandem UI (Tauri + Svelte)         │        │ Tandem UI                          │
│        │ local IPC                 │        │        │ local IPC                 │
│ tandemd                            │◄──────►│ tandemd                            │
│  ├ link    (paths, health, QoS)    │ QUIC   │  ├ link                            │
│  ├ input   (capture / injection)   │ encrypt│  ├ input                           │
│  ├ clipboard + files (drag)        │        │  ├ clipboard + files               │
│  ├ tunnels (processes, files,      │        │  ├ tunnels                         │
│  │          ports, workspaces)     │        │  │                                 │
│  ├ monitors (monitor-ddc)          │        │  ├ monitors                        │
│  ├ desk    (shared config,         │        │  ├ desk                            │
│  │          scenes, arrangement)   │        │  │                                 │
│  ├ power, peripherals, resources   │        │  ├ …                               │
│ tandem mcp / tandem (CLI) ── IPC ──┘        │                                    │
└────────────────────────────────────┘        └────────────────────────────────────┘
```

### 4.1 Stack

- **`tandemd` agent:** Rust, edition 2024, tokio. Direct FFI to each system.
- **UI:** Tauri 2 + **Svelte 5 + Vite + TypeScript** (A3). Tokens, buttons, icons and animations from the v0.3 mockups.
- **Protocol:** typed messages (serde) over QUIC (quinn + rustls), Ed25519 identities.
- **Claude clients:** `tandem mcp` (MCP server over stdio) and `tandem` (CLI), both from `tandem-cli`, both talk to the local `tandemd` over IPC.

### 4.2 Workspace

```
crates/
  tandem-core        domain model: Machine {A,B}, shared config, scenes, transactions
  tandem-proto       messages between agents and with the UI/CLI, versioned (§4.5)
  tandem-link        discovery, paths, health, priority and QoS
  tandem-input       keyboard and mouse engine
  tandem-clipboard   clipboard
  tandem-files       file drag and transfers (chunks, hash, resume), tunnel files and ports
  tandem-tunnel      tunnels: sessions that survive the link, processes with per-byte output position
  tandem-workspace   per-tunnel git workspaces: the repo travels as a bundle plus a patch and comes back as a branch (`bring_back`)
  tandem-env         detected environments (shells, WSL, Docker, toolchains) and live resources
  tandem-monitor     monitor-ddc integration: owner, events, changes with verdict
  tandem-peripherals keyboard and mouse detection (activity)
  tandem-power       Wake-on-LAN, power off
  tandem-cli         `tandem` (CLI) and `tandem mcp` (MCP server over stdio): clients of the agent's IPC
  tandem-sim         simulated desk for tests: two agents in one process over loopback
  tandem-hw          hardware test suite (never in CI): provisions test agents, runs the checks over IPC, builds the report
  tandemd            agent binary (launchd service / Windows Service), including the secure-desktop service and helper
apps/
  tandem-ui          Tauri 2 + Svelte 5
tools/
  mockup-render      local renderer of mockups/v03 to compare screenshots
  windows/installer  builds the Windows NSIS installer (`build-installer.ps1`) and tests it (`test-installer.ps1`)
```

Own external dependency: `monitor-ddc` (repo `monitor-firmware`, A5), pinned by commit.

### 4.3 Capabilities and modules

- Each module is compiled only where it can exist (`#[cfg]`) and at runtime reports a **state**: `active`, `off` (by the user), `unavailable` (with a reason: missing permission, missing hardware, not supported).
- `ControlMsg::Capabilities` carries: system, name, color, modules with state, detected environments, resources and monitors with a DDC channel. It is sent on connect and on every change.
- The UI shows every module of each machine with its state and reason (mockup `V3-Machine`); nothing is hidden without explanation.

### 4.4 Shared configuration

- A **desk document** (`Desk`) with sections: machines (name, color), arrangement, scenes, shortcuts, keyboard/mouse preferences, enabled modules. It carries a version (`desk_rev`) and author.
- The UI edits over IPC (`UiCommand::EditDesk { patch }`); the agent applies it, bumps the version, saves to `tandem.toml` and sends it to the other machine.
- **Last write wins** (D5), except right after both machines migrate an old desk at once: each keeps its own moved settings (DECISIONS T-322). The other UI shows "Changed from <machine>" with **Undo** (history of the last 20 versions).
- Each machine's local data (paths, permissions, environments) is not synced.

### 4.5 Protocol versioning

- **Current version: `PROTOCOL_VERSION = 50`** (`crates/tandem-proto/src/lib.rs`, last bumped by T-356: `MonitorChannel::size_mm` and `MonitorView::size_mm`, each monitor's physical size from its EDID, so the UI draws it at its real proportions; v49 was terminal tunnels: `ExecSpec::pty`, `TunnelUp::Input`/`Resize`, `UiCommand::Input`/`Resize`/`WatchOutput`, `wait_ms` in `UiCommand::Output` and the pushed `AgentToUi::Output`, with `Step::MonitorPower` / `UiCommand::MonitorPower` (monitor standby and wake by command) together with T-353, `DeskPatch::BuiltinDisplay` and the desk's `builtin_display`; v48 was T-349, `Capabilities::cables` and `MonitorView::link`; v47 was T-350, `AgentToUi::ShowWindow`; v46 was T-341, `Step::DisplayOn`; v45 was T-346, `MonitorReport::cursor`; v44 was T-344, `Capabilities::local_network` and `UiCommand::CheckLocalNetwork`; v43 was T-345, `PairingStatus::Offered`).
- **Rule: only the same version talks.** The two agents and the UI ship together; an agent that sees another version refuses the link and says to install the same Tandem version on both machines. Bump `PROTOCOL_VERSION` on any incompatible change to the messages (almost every release does).
- **Hints for older peers** (only to word the refusal; they do not make anything compatible): `CAPABILITIES_SINCE = 10` is the first version where each agent announces its capabilities on connecting (T-084); `SIDES_SINCE = 16` is the first where the two sides are `A`/`B` ordered by public key (T-097), instead of "Mac" and "PC".
- **History:** the entries of [DECISIONS.md](DECISIONS.md) that say "Protocol vN" record what each bump changed (for example v18, v30, v33, v35, v39).
- **Independent versions:** the secure-desktop pipe between the session agent and the helper has its own version, currently 1 (`PROTOCOL_VERSION` in `crates/tandemd/src/secure/protocol.rs`), because the service's copy of `tandemd` is only refreshed by an elevated install; see [dev/secure-desktop.md](dev/secure-desktop.md). It does not move with the agent protocol.
- **Keeping this section true:** a PR that changes any of these constants must update the numbers above; `scripts/ci/protocol.sh` (run in CI) fails if they differ from the code.

## 5. Scenes and power

- **Scene** = which input each supported monitor shows + desktop arrangement + (optionally) a machine that must be powered on. Examples: "Work" (34" on the PC, MacBook on the left), "MacBook only", "PC only".
- **Applying a scene** is a transaction: power on the machine if needed → switch each monitor (with the firmware's verdict) → apply the arrangement → move the input engine's geometry. If a step fails, it rolls back. Mockup `F-Transactions`.
- Unsupported monitors do not take part in scenes (they are shown, but cannot be assigned).
- **Power on** the other machine: Wake-on-LAN from the one that has its MAC and a LAN path; wait for its agent (120 s).
- **Power off** the other: a transaction (release keys, clean shutdown). macOS only if it announces it can.
- **Peer watch:** if no path sees it for `peer_lost_after_secs`, it is considered off and a notice is shown.

## 6. Network: paths and priorities

| Path | Priority | Input | Data (tunnels, files) |
|---|---|---|---|
| Direct USB-C (USB4 / Thunderbolt network) | 1 | ✅ | ✅ |
| Direct Ethernet | 2 | ✅ | ✅ |
| Wi-Fi (LAN) | 3 | ✅ | ✅ capped (A2) |

- **Input rules:** keyboard and mouse go in datagrams and in a top-priority stream; data goes in low-priority streams. On Wi-Fi, data has a configurable bandwidth cap (by default 50 % of the measured one). On any path data is paused when the input p99 stays over 150 % of its target for 2 s; a pause lasts at most 2 s (it ends earlier once the p99 is back within the target for 1 s) and data then runs at least 10 s before the next one, so the pause is a short brake and never a state.
- **Health and path switching:** each backup path is pinged once per second and the active one every 200 ms; a ping waits in 200 ms windows and the path is down after 2 windows in a row without the Pong (a single late Pong does not switch), or at once on a connection error. With no healthy backup to move to, the active path's ping waits up to 1 s instead (a drop would gain nothing and cost a reconnect). A backup's ping waits up to 2 s, since a backup carries nothing and Wi-Fi's round trip spikes to hundreds of milliseconds now and then; a backup whose ping is late past 400 ms is not switched to and restarts its hysteresis. A path is identified by its kind (and, for a cable, its interface), not by its address: a new address for a healthy path (mDNS answers that come and go) is kept for the next dial and never cuts its connection; only the interface going away removes a cable. A cut is noticed at most 600 ms after the path's last answer, and input moves to the next healthy path right then: under 1 s (T-310). Hysteresis when going back (T-065): a path that dropped is gone back to after 3 s of good pings, doubled for each further drop within 30 min, up to 10 min (T-364), because each switch restarts the input session and a cable that keeps dropping must not drag the keyboard and mouse back and forth. The count survives the interface going away and coming back.
- **Input rules:** keyboard and mouse go in datagrams and in a top-priority stream; data goes in low-priority streams. On Wi-Fi, data has a configurable bandwidth cap (by default 50 % of the measured one) and is paused if input latency exceeds its target. The output of tunnel processes (what a terminal shows) goes at data priority and respects the cap, but is not paused: a pause froze terminals for up to 90 s while input was barely late.
- **Health and path switching:** each backup path is pinged once per second and the active one every 200 ms; a ping waits in 200 ms windows and the path is down after 2 windows in a row without the Pong (a single late Pong does not switch), or at once on a connection error. With no healthy backup to move to, the active path's ping waits up to 1 s instead (a drop would gain nothing and cost a reconnect). A cut is noticed at most 600 ms after the path's last answer, and input moves to the next healthy path right then: under 1 s (T-310). Hysteresis when going back (T-065).
- **One-way reachability (T-237):** a firewall (macOS stealth mode, for example) can drop what one side sends while the other can still dial in. Each side serves the streams of every authenticated connection with the paired peer, whoever opened it; for each path, a side first adopts the live connection the other agent opened over it and only dials when there is none. Simultaneous dials leave two connections, each side keeps the one its session already uses, and neither closes the other's.
- **Per-path metrics** for the Network screen: latency p50/p99 (input) and RTT, bandwidth measured on demand, 60 s history, switch events ("moved to Wi-Fi in 0.6 s"). Mockup `V3-Network`.
- **USB-C (T-094):** automatic interface detection (Thunderbolt Bridge on macOS; "USB4 P2P Network Adapter" on Windows), link-local addresses, mDNS only on that interface, classification of each end (the worst one counts). Today without a physical link (§2).
- Wake-on-LAN only over wired LAN or Wi-Fi with the machine asleep (not over USB-C).

## 7. Keyboard and mouse (own engine)

- **No owner (D7):** both machines capture; the one receiving physical input is the one in charge at that moment ("follows the one you use"). If physical input arrives on both at once, the one that moved last wins after 300 ms of stillness on the other.
- **Capture:** `CGEventTap` on macOS; low-level hooks / Raw Input on Windows. With focus on the other machine, events are swallowed and the local cursor is hidden and locked.
- **Same feel on both machines (T-358):** the movement that travels carries the pointer speed and acceleration of the machine the mouse is plugged into. macOS gives it accelerated (`kCGMouseEventDeltaX/Y`); on Windows, with input swallowed, it is the point the mouse hook reports (where Windows would have taken the cursor, with "Enhance pointer precision") minus where the frozen cursor is, not Raw Input's counts. On grabbing, the Windows cursor waits in the middle half of its monitor, so the hook's point is never clamped at the edge.
- **A cursor without a mouse (T-358):** Windows does not draw the cursor when it has no mouse (the hub is on the other machine), and injected input does not change that. While the focus is on the PC as client and Windows is not drawing the cursor, the injector turns MouseKeys on (only in the Num Lock state the user is not typing with, without saving it in the profile) and puts the user's setting back on the Leave or when the session ends.
- **Injection:** `CGEventPost` on macOS; `SendInput` with scancodes on Windows. It runs on its own high-priority thread (`ThreadedInjector`, T-358): the network loop never waits for it, and while it is busy only the newest position waits (keys and buttons keep their order), so no queue of old positions can build. On Windows plain movement goes with `SetCursorPos` (~10 µs against 0.4–1.5 ms for `SendInput`, which waits for every low-level hook); with a button held and once every 500 ms, as input.
- **Keys:** canonical physical code (USB HID); respects each system's ES/LatAm layout. Cmd↔Ctrl configurable.
- **Native shortcuts (T-361):** with `cmd_ctrl` on, the server also rewrites the chords of the source system into the intent's equivalent at the target (`tandem_input::event::shortcuts`, pure, applied where the modifier remap is). PC→Mac: Ctrl+Left/Right/Backspace/Delete → Option+..., Home/End → Cmd+Left/Right, Ctrl+Home/End → Cmd+Up/Down (Shift variants included), Alt+Tab → Cmd+Tab (Cmd held while Alt is), Alt+F4 → Cmd+Q, Win tap → Cmd+Space, Win+D → F11, Win+L → Ctrl+Cmd+Q, Win+E → Cmd+Option+Space, PrintScreen → Cmd+Shift+3, Win+Shift+S → Cmd+Shift+4, Ctrl+Shift+Esc → Cmd+Option+Esc. Mac→PC is the mirror (Option+Left → Ctrl+Left, Cmd+Left → Home, Cmd+Tab → Alt+Tab, Cmd+Q → Alt+F4, Cmd+Space → Win, Ctrl+Cmd+Q → Win+L, Cmd+Shift+3/4 → PrintScreen / Win+Shift+S, Cmd+Option+Esc → Ctrl+Shift+Esc). A translation lives as long as its key is held and releasing everything at the source leaves nothing pressed at the target; unmapped chords keep the plain Cmd↔Ctrl swap. AltGr / right Option is never translated (it types characters). The lock shortcut's keys and the scenes' shortcuts match the physical chord, before translation. Cmd+Backspace and F2 are not translated.
- **Transport:** movement in unreliable datagrams (coalesced); keys and clicks in a reliable top-priority stream.
- **Geometry: declared arrangement** (D18). Each machine declares its screens (rectangles in native cursor coordinates and scale); the arrangement says on which side (top, bottom, left, right) the other's desktop is and with what alignment (start, center, end). The crossing edge is the shared stretch. By default, the limits the system already gives when injecting.
- **Scenes and geometry:** a screen whose monitor does not show this machine is left out of the geometry: it shows the other machine, or its link state (§10) is off, standby or an unplugged input (T-349); an unknown state keeps it. A local cursor left in a screen that leaves the geometry is moved to the nearest live screen (`ServerOutput::Warp`). If every screen of the machine with the keyboard shows the other one (a scene, `Layout::shown_elsewhere`; screens not read yet do not count), the cursor goes to the other machine by itself, to the centre of its first screen, and it never comes back to a machine with no visible screen: there it would be invisible and, with no crossing edge, stuck (T-358). Which screen a monitor is on each machine is the `screen` of each end of its `[[cables]]`; each agent fills its own end by linking the monitor's EDID (manufacturer, product, serial) to the screen the system reports with the same identity (CoreGraphics on macOS, the EDID of the display path on Windows), even when that machine has no DDC channel to it (T-340). An end already filled, or a machine with `[[screens]]` declared, is never touched. The UI's live desktops draw a shared monitor once, on the machine it shows.
- **Clipboard:** text, HTML and image. **Files:** dragging a file across the edge transfers it (`tandem-files`) and drops it at the destination (§9).
- **Windows, secure desktop** (T-027, done: implemented and tested in CI; verification on the real PC with UAC pending): SYSTEM service + helper for UAC and the lock screen. Design and manual test: [dev/secure-desktop.md](dev/secure-desktop.md).
- **An elevated window without the service (T-363):** UIPI keeps the medium-integrity agent from moving the cursor or injecting while a higher-integrity window (Task Manager, an installer) has the foreground. Without a helper the failures form one episode: one log line naming the process when it starts, a counter while it lasts, one line when injection works again; the "Protected screens" module names the process ("<process> runs as administrator; Tandem cannot control it without the protected screens service"). The first failure of an episode asks for the helper at once, so a service installed a moment ago is used without waiting for the next retry. `--install-service` tries the start again up to five times, 2 s apart, when Windows refuses it right after the copy (an antivirus scanning the new file).
- Engine design: [research/input-engine.md](research/input-engine.md).

### 7.1 Latency and reliability

| Path | p50 | p99 |
|---|---|---|
| USB-C / Ethernet | ≤ 1 ms | ≤ 4 ms |
| Wi-Fi | ≤ 5 ms | ≤ 20 ms |

Measurement always available (monotonic timestamps, offset estimated with the heartbeats), a bench in `tandem-sim`, a hot path without shared queues, and the known failures covered (hook removed by Windows, tap disabled on macOS, stuck keys, per-screen DPI). Details in T-093.

## 8. Tunnels (Claude uses the other machine)

- **Tunnel** = a session between the two machines opened by a client (Claude through MCP or a person through the CLI), with id, title (the Claude session's), project (repo/folder) and destination machine. Several tunnels at once; several processes per tunnel.
- **Inside a tunnel:**
  - `exec`: launch processes (shell chosen from the detected environments: zsh/bash, PowerShell, cmd, WSL, Docker), with cwd and variables; several at once. With `pty` the process runs in a pseudo-terminal (openpty on Unix, ConPTY on Windows): it sees a TTY (colors, progress, line editing, line-buffered output) and stdout and stderr come together.
  - `input` / `resize`: type into a terminal process and change its size. Each byte carries its position, so input arrives in order and exactly once, also when a reply is lost or the link moves.
  - `output`: live output (stdout/stderr) with a circular buffer; `kill`; exit code. `output` can wait (`wait_ms`) and answers the moment a byte arrives; the UI is pushed the output of the process it shows (`WatchOutput`), each piece the exact continuation of the previous one.
  - `files_put` / `files_get`: copy files and folders (chunks with hash, resumable).
  - `port_forward`: a local port that reaches one on the other machine (or the reverse).
  - **Git workspace:** `tunnel_open` with `workspace` copies the repo (current branch + uncommitted changes, via `git bundle` + patch) to a working folder on the other machine; `bring_back` brings the commits made there back as a `tandem/<tunnel>` branch in the origin repo.
- **They survive the link:** processes live in the destination agent; if the link drops, they keep running and the output is saved; when it returns, the client resumes from the last byte received.
- **Privileges (A7):** the agent's (elevated on Windows; user on macOS).
- **Environments and resources (`tandem-env`):** scan at startup and on demand: shells, WSL distros, Docker, git, toolchains (cargo, node, uv/python), GPU (model, VRAM, usage via `nvidia-smi` or Metal). Live resources: CPU, RAM, GPU, disk. Claude picks the machine using this data.
- **MCP (`tandem mcp`):** tools `machines`, `resources`, `tunnel_open`, `tunnel_close`, `tunnels`, `exec`, `input`, `resize`, `output`, `kill`, `files_put`, `files_get`, `port_forward`, `bring_back`. Mockup `F-ClaudeMCP`. The `tandem` CLI exposes the same.
- **UI (read-only, D9):** grouped projects → tunnels (title, branch, path A→B) → running processes and their output. Mockup `V3-Compute`. The output is a terminal screen (xterm.js) fed with the pushed bytes: it only appends, and desk updates or a link switch never clear it.

## 9. Files: dragging between machines

- When crossing the edge with a file drag in progress (Finder / Explorer), the source machine reads the dragged paths, `tandem-files` transfers them and the destination starts an equivalent drag with the files already copied into a temporary folder; on drop, the destination system does the normal copy.
- If the drag is cancelled, what was transferred is deleted. Progress in the UI (notice with a bar).
- It requires crossing with the button pressed: this feature enables crossing during a file drag even if "Drag between machines" is off for everything else.

## 10. Monitors (single standard)

- **Standard:** the protocol of the 34"'s own firmware (MCCS + VCP `0xEA`–`0xEF` events and verdicts, `0xEE` live state, `0x60` with the real input). A monitor is **supported** if the probe (`Get 0xEA`, reads only) confirms it. Any other: **not supported** (shown, not controlled). Vendor codes are never sent without a prior probe.
- **`monitor-ddc` crate** (repo `monitor-firmware`, A5): `DdcBus` trait (Get/Set VCP, capabilities, EDID), Windows (`dxva2`) and macOS (`IOAVService` on Apple Silicon) backends, a test double with personalities (standard, factory, disconnected), probe, event reading with a cursor, input change with verdict.
- **Present without a channel (T-340):** a monitor whose EDID the system lists but which does not answer the probe (the 34" only answers DDC on the input it shows) is still announced, as `NoChannel`: it gives that machine's input and screen, never takes part in the owner election and is never opened or written to. So the desk shows one monitor with both connections even while only one machine can talk to it.
- **No phantom display (T-341):** on macOS, while a monitor shows the other machine, the Mac turns off its own display of that monitor (the private `CGSConfigureDisplayEnabled`, resolved with `dlsym`, `kCGConfigureForSession`), so its trackpad and its windows do not go into a screen nobody sees. What each monitor shows comes from the owner's real input (the same `shows` as the geometry, §7); the display goes back on when the monitor shows the Mac again, or when its input has not been known for 15 s. Before any input change to a machine's input (scene, UI selector or rollback), the orchestrating agent asks that machine to turn its display back on (`Step::DisplayOn`, never part of a plan, best effort), so the monitor never lands on a dark input; the display then stays on for up to 30 s while the change lands. It never turns off the only screen, nor one that is not the last in the order screens are numbered (`screen-N`): that would renumber the others and the cables would name the wrong screen. What it turned off is saved in `displays-off.json` next to the state before each change and turned back on at exit (normal end, signal, panic hook) and at start-up (after a crash). A macOS without the symbol works as before and the "Monitors" module says display control is unavailable. Windows cannot do this (T-360): the monitor keeps its DP alive while it shows HDMI, so Windows keeps that display in its desktop, and it is not detached because the PC is the only DDC/CI path to the Odyssey (the Mac reaches it through a USB-C to HDMI adapter without DDC) and a detached display has no `HMONITOR`, so nobody could switch the monitor back. Instead there is a **window rescue** (`tandem_input::rescue`, Win32 behind `WindowMover`; a no-op on macOS, which moves its windows itself): while a screen of this machine belongs to a monitor that shows the other machine (the same `shows` signal as the display guard), every user top-level window mostly on that screen moves to the primary visible screen (else the first), size clamped to its work area and maximized state kept; a minimized window gets its restore rectangle (`rcNormalPosition`) moved. It runs on each change and once a second. Each moved window's original placement is remembered and put back when the screen is seen again, unless the window is gone or the user moved it. Invisible, cloaked, tool and owned windows, the shell and Tandem's own are never touched; a window of an elevated process (UIPI) fails once, is logged once and is not retried. With no visible screen nothing is done.
- **Link state (T-349):** each shared monitor has one derived state (`tandem_core::link`, pure): `ShowsMachine(m)`, `Standby`, `Off`, `ActiveInputUnplugged(m)` (the input shown leads to a machine whose cable is disconnected) or `Unknown`, plus each end's cable (`connected` when its system lists the EDID, `disconnected` when it is reachable, has just rescanned and does not list it, `unknown` otherwise) and the power (`on`, `standby` without power or without signal, `off` after `OFF_AFTER` = 10 s of silence from a probed channel on the shown input while a cable still lists the EDID, `unknown`). A monitor whose ends are all `NoChannel` is never `off`. Safety rule until hardware confirms both sides: a machine that does not list a monitor showing the other machine reports `unknown`, never `disconnected`. Each agent announces its own ends in `Capabilities::cables`; the views carry `MonitorView::link`. A display change on the system (macOS `CGDisplayRegisterReconfigurationCallback` plus a 250 ms online-display check on its own run-loop thread; Windows `WM_DISPLAYCHANGE` and the monitor-interface `WM_DEVICECHANGE` on a hidden window, behind `tandem_input::hotplug::DisplayChanges`) rescans at once. Acting on it: the geometry (§7); the Mac turns off its display of a monitor that is off, like one that shows the other machine (T-341); switching to an input whose machine's cable is `disconnected` is refused before any DDC write, by the selector and by a scene's preflight; the UI says it in plain words ("Showing PC", "Mac cable unplugged", "Monitor in standby", "Monitor off") on Desk, Monitors and the tray, draws such a monitor's live desktop dimmed, and announces each transition once. `tandem monitors` (CLI and MCP) returns it.
- **No phantom mouse after a role change (T-362):** a server session always starts with the geometry current at that moment (the driver only follows it while serving, so after idling it applies the pending layout before `start()` settles the cursor); if the capture refuses to take the input (macOS Secure Event Input: a password field, the lock screen) the session goes back to local and tells the client (`ServerSession::grab_refused`), never believing the focus is on a machine the local input did not leave; and the client's injector rereads the screens the system reports whenever they change (`ClientDriver::with_display_changes`), also when the geometry is made of declared screens and does not change. Becoming sender with Secure Event Input active is still allowed: the crossing is refused and the machine stays local, which is what the user sees anyway. A focus that went to the other machine only because this one had no visible screen comes back as soon as one of its screens is visible again; one that crossed by an edge stays.
- **Protocol 3 and power by command:** the 34" (v3.2) and the 32" G5 (its production image) speak protocol 3 of `monitor-ddc`: a capability word (`0xE8`), soft standby and cable detect. Each monitor is opened with its model's `0x60` values (the G5 uses MCCS `0x0F`/`0x11`, from the crate's profiles), and its events and direct readings decode with them. `Step::MonitorPower` puts a monitor in standby (`0xD6 = 4`) or wakes it (`0xD6 = 1`); its owner does it, and only when the firmware announces power wake, because without it the monitor may drop its link and only a signal or its button wakes it. Soft standby is reported as power off, so the link state says "standby" and the screen leaves the geometry. The UI's Monitors screen has a Standby/Wake button; the CLI/MCP has `monitor_switch` and `monitor_power`.
- **Renumbered screens:** a cable end learned as an engine id (`screen-N`) is learned again when the EDID identity puts the monitor on another screen (turning the Mac's built-in display off or mirroring it renumbers the screens); a name a person gave is never replaced.
- **Every display by its EDID and the built-in display (T-353):** a monitor whose E-EDID 1.3 does not say the interface gets its input from the model's per-input product id (`monitor-ddc`'s `host_input`). On macOS every external display of the IOKit registry (`IOPortTransportStateDisplayPort`, `EDID`) that the DDC bus does not enumerate is announced too, as `NoChannel`. The desk's `[builtin_display]` sets per machine what a Mac does with its built-in display while an external display is active: `external-only` (default: off), `mirror` (of the main external display) or `extended` (as macOS leaves it; only Tandem's change is undone). Without an active external display it is always on, and with the lid closed it is never touched. What it was is saved in `builtin-display.json` and put back like T-341's displays. A display macOS refuses to turn back on is reported once and retried with a doubling wait (1 s to 60 s), also by its EDID identity in case its id changed.
- **Owner per monitor (D13):** each agent reports which monitors it sees with a live DDC channel. The owner is the one with a channel; if both, the one with the lower response latency; if the owner leaves or loses the channel, the other is chosen. Only the owner reads events (the event cursor is global to the monitor).
- **The other machine with a channel (T-346):** the 34"'s DP and HDMI DDC/CI share one request buffer, one pending flag and one reply length, and the event cursor is global (`monitor-firmware`, `docs/targets/lc34g55t/inactive-hdmi-ddc.md`): if both machines talk at once, either can get the other's reply. So the machine that has a channel but is not the owner never talks to the monitor while the owner is reachable over the control plane (beyond the channel probe of every scan): it shows the owner's report. While the owner is unreachable but not yet given up for lost (`peer_lost_after_secs`), it reads the live state itself on every poll, `Get 0xEE`, `0x60`, `0xEE` and never an event register, and can switch the input itself (`Set 0x60` with the firmware's verdict, rolled back like the owner's). Every reply is validated: the backend drops one whose echoed code is not the one asked (`monitor-ddc`'s `parse_vcp_reply` on macOS), and a direct reading counts only if both `0xEE` replies are equal, say DDC/CI is enabled and name the same input as `0x60`; anything else is dropped, never shown or acted on. That reading is shown over the owner's report until the owner reports again. When the owner is given up for lost, the monitor passes to the other machine in the same poll, and it reads the events from the cursor the owner last reported (`MonitorReport::cursor`), so nothing that happened in between is lost.
- **Identity:** `<model>-<short hash of the EDID serial>`; it links the DP id and the HDMI id (the EDID changes per input). A monitor is linked by its EDID, with or without a DDC channel: the same EDID also links it to the machine's screen (§7).
- **Input changes:** a transaction with the firmware's verdict (`confirmed` / `failed`), timeout and rollback. In hardware tests, **automatic revert is mandatory**.
- **UI:** monitors with their real input, owner and events; input selector with a sliding indicator; unsupported ones in their own card. Mockup `V3-Monitors`. Each monitor carries its picture's physical size from its EDID (`MonitorView::size_mm`: the first detailed timing descriptor in mm, else bytes 21–22 in cm; T-356): the UI names it by its diagonal (`34"`, `32"`) and the Desk's live desktops draw each cabled screen at that size, with its model's stand when the EDID names a known one.

## 11. Security

- Pairing with a 6-digit code on both UIs; Ed25519 identities in the system keychain.
- Mutual authentication on every connection; local IPC only for the current user.
- Tunnels: only a local client authenticated over IPC opens them (the MCP and the CLI run as the user); the other machine only runs what arrives over the authenticated link.

## 12. UI

- **Shell:** sidebar (Desk, Tunnels, Monitors, Arrangement, Network; Machines: one page for each; Settings at the bottom). Menu bar / tray with a mini summary.
- **Screens** (reference mockup in `mockups/v03/`):
  | Screen | Mockup |
  |---|---|
  | Desk (home): machines, live link, live desktops at real scale, scenes | `V3-Desk` |
  | Tunnels (read-only) | `V3-Compute` |
  | Machine: resources (CPU, RAM, GPU, disk), modules, environments | `V3-Machine`, `F-MachineMac` |
  | Monitors and scenes | `V3-Monitors`, `F-NewScene` |
  | Arrangement (4 sides + alignment) and crossing test | `V3-Arrangement`, `F-CrossingTest` |
  | Network | `V3-Network` |
  | Setup (6 steps) | `F-Setup1`, `V3-Setup`, `F-Setup3`…`F-Setup6` |
  | Scene/power in progress, notices | `F-Transactions`, `F-Notices` |
  | Settings (tabs with icons), ⌘K palette | `F-Settings`, `F-Palette` |
  | Trouble states (no agent, no other machine…) | `V3-States` |
  | Menu bar / tray | `V3-Tray` |
- **Mockup parity:** every screen is captured in demo mode and compared with the mockup's render (`tools/mockup-render`); target ≥ 99 % visual similarity, with the same animations and transitions.
- **Visual rules:** dark and sober; accent per machine; a single button system (fixed height, always an icon, elevation on hover, glow on the primary, visible focus); icons everywhere; animations and transitions everywhere, respecting `prefers-reduced-motion`; no colored lines on cards; no superfluous explanatory text; desktops at real scale.
- **Demo mode:** simulated data for every screen and every state, for screenshots and tests.

## 13. Release

- Installers on GitHub Releases when a `v*` tag is pushed: `.dmg` (macOS) and NSIS (Windows). No Developer ID, notarization or Authenticode. **macOS builds are signed with a stable self-signed code-signing identity** (generated once, kept as a repo secret; DECISIONS T-348), not ad hoc: the designated requirement (identifier + certificate leaf) stays the same across versions, so macOS keeps Accessibility, Input Monitoring and Local Network after an update; CI checks it (`scripts/macos/dr-check.sh`). Gatekeeper still treats the first install as from an unidentified developer. Windows stays unsigned.
- **The `tandem` CLI and MCP server ships inside both installers** (T-357): `Tandem.app/Contents/MacOS/tandem` and `%LOCALAPPDATA%\Tandem\tandem.exe`, built from the same commit as the agent, so an update never leaves the MCP on another protocol version (on 2026-11-24 a `tandem` installed apart with `cargo install` stayed on protocol 47 and the 0.3.3 agent refused it). MCP clients point at that path.
- **Automatic update:** `tauri-plugin-updater` with the updater's own signature (minisign; not code signing) against GitHub Releases; it updates the UI and the agent; the two machines update one at a time without cutting the keyboard. One click: the app shows "Update available: vX.Y.Z" with one button that runs "Update all" and then the result (T-348).

## 14. Risks and spikes

| # | Spike | Status |
|---|---|---|
| S5 | USB4 network Mac↔Windows (ASM4242) | Negative with the 10 Gbps cable; try a TB4/USB4 40 Gbps cable |
| S6 | Wake-on-LAN from power-off on the X870 | Pending (needs wired LAN) |
| S7 | macOS permissions | Done (T-090) |
| S9 | Windows secure desktop | Implemented (T-027); hardware verification with UAC pending, and the Authenticode decision |
| S11 | Real capture → injection latency | T-093 |
| S12 | DDC from the Mac over HDMI (`IOAVService`) | Only with active HDMI; to be tested |
| S13 | File drag between systems (start a synthetic drag at the destination) | T-121 |

## 15. Phases (v0.3)

| Phase | Content | Tasks |
|---|---|---|
| H1 Foundations | A/B sides, capabilities and modules, shared config, scenes | T-097…T-100 |
| H2 Input and network | Latency, data over any path, metrics, USB-C | T-093, T-101, T-102, T-094 |
| H3 Tunnels | Sessions, processes, environments, files and ports, workspaces, MCP/CLI | T-103…T-108 |
| H4 Monitors | `monitor-ddc`, integration, scenes with monitors | T-109…T-111 |
| H5 Live UI | New base and all screens, mockup renderer | T-112…T-120, T-125 |
| H6 Files | Drag between machines | T-121 |
| H7 Release | Installers and automatic update | T-122 |
| QA | Overnight hardware, end-to-end acceptance | T-123, T-124 |

## 16. Development flow

- Private repo `franc1sc0sv/tandem`, integration on `develop` (never `main`). CI: Linux, Windows, macOS + UI.
- Autonomous work: [dev/night-run.md](dev/night-run.md) (captain, implementers, reviewers, acceptance, QA, hardware).
- Tests: [TESTING.md](TESTING.md).

## 17. Open questions

- Does the 40 Gbps TB4/USB4 cable bring up the link with the X870's current firmware? (S5)
- Does the 32" come back with factory firmware or with the development one? (decides whether it is "not supported" or joins the standard)
