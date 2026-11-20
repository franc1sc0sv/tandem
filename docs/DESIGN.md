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
- **Last write wins** (D5). The other UI shows "Changed from <machine>" with **Undo** (history of the last 20 versions).
- Each machine's local data (paths, permissions, environments) is not synced.

### 4.5 Protocol versioning

- **Current version: `PROTOCOL_VERSION = 41`** (`crates/tandem-proto/src/lib.rs`, last bumped by T-240, `KeyProbeMsg::Armed`).
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

- **Input rules:** keyboard and mouse go in datagrams and in a top-priority stream; data goes in low-priority streams. On Wi-Fi, data has a configurable bandwidth cap (by default 50 % of the measured one) and is paused if input latency exceeds its target.
- **Health and path switching:** each backup path is pinged once per second and the active one every 200 ms; a ping waits in 200 ms windows and the path is down after 2 windows in a row without the Pong (a single late Pong does not switch), or at once on a connection error. With no healthy backup to move to, the active path's ping waits up to 1 s instead (a drop would gain nothing and cost a reconnect). A cut is noticed at most 600 ms after the path's last answer, and input moves to the next healthy path right then: under 1 s (T-310). Hysteresis when going back (T-065).
- **One-way reachability (T-237):** a firewall (macOS stealth mode, for example) can drop what one side sends while the other can still dial in. Each side serves the streams of every authenticated connection with the paired peer, whoever opened it; for each path, a side first adopts the live connection the other agent opened over it and only dials when there is none. Simultaneous dials leave two connections, each side keeps the one its session already uses, and neither closes the other's.
- **Per-path metrics** for the Network screen: latency p50/p99 (input) and RTT, bandwidth measured on demand, 60 s history, switch events ("moved to Wi-Fi in 0.6 s"). Mockup `V3-Network`.
- **USB-C (T-094):** automatic interface detection (Thunderbolt Bridge on macOS; "USB4 P2P Network Adapter" on Windows), link-local addresses, mDNS only on that interface, classification of each end (the worst one counts). Today without a physical link (§2).
- Wake-on-LAN only over wired LAN or Wi-Fi with the machine asleep (not over USB-C).

## 7. Keyboard and mouse (own engine)

- **No owner (D7):** both machines capture; the one receiving physical input is the one in charge at that moment ("follows the one you use"). If physical input arrives on both at once, the one that moved last wins after 300 ms of stillness on the other.
- **Capture:** `CGEventTap` on macOS; low-level hooks / Raw Input on Windows. With focus on the other machine, events are swallowed and the local cursor is hidden and locked.
- **Injection:** `CGEventPost` on macOS; `SendInput` with scancodes on Windows.
- **Keys:** canonical physical code (USB HID); respects each system's ES/LatAm layout. Cmd↔Ctrl configurable.
- **Transport:** movement in unreliable datagrams (coalesced); keys and clicks in a reliable top-priority stream.
- **Geometry: declared arrangement** (D18). Each machine declares its screens (rectangles in native cursor coordinates and scale); the arrangement says on which side (top, bottom, left, right) the other's desktop is and with what alignment (start, center, end). The crossing edge is the shared stretch. By default, the limits the system already gives when injecting.
- **Scenes and geometry:** screens that show the other machine in the active scene are left out of the geometry.
- **Clipboard:** text, HTML and image. **Files:** dragging a file across the edge transfers it (`tandem-files`) and drops it at the destination (§9).
- **Windows, secure desktop** (T-027, done: implemented and tested in CI; verification on the real PC with UAC pending): SYSTEM service + helper for UAC and the lock screen. Design and manual test: [dev/secure-desktop.md](dev/secure-desktop.md).
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
  - `exec`: launch processes (shell chosen from the detected environments: zsh/bash, PowerShell, cmd, WSL, Docker), with cwd and variables; several at once.
  - `output`: live output (stdout/stderr) with a circular buffer; `kill`; exit code.
  - `files_put` / `files_get`: copy files and folders (chunks with hash, resumable).
  - `port_forward`: a local port that reaches one on the other machine (or the reverse).
  - **Git workspace:** `tunnel_open` with `workspace` copies the repo (current branch + uncommitted changes, via `git bundle` + patch) to a working folder on the other machine; `bring_back` brings the commits made there back as a `tandem/<tunnel>` branch in the origin repo.
- **They survive the link:** processes live in the destination agent; if the link drops, they keep running and the output is saved; when it returns, the client resumes from the last byte received.
- **Privileges (A7):** the agent's (elevated on Windows; user on macOS).
- **Environments and resources (`tandem-env`):** scan at startup and on demand: shells, WSL distros, Docker, git, toolchains (cargo, node, uv/python), GPU (model, VRAM, usage via `nvidia-smi` or Metal). Live resources: CPU, RAM, GPU, disk. Claude picks the machine using this data.
- **MCP (`tandem mcp`):** tools `machines`, `resources`, `tunnel_open`, `tunnel_close`, `tunnels`, `exec`, `output`, `kill`, `files_put`, `files_get`, `port_forward`, `bring_back`. Mockup `F-ClaudeMCP`. The `tandem` CLI exposes the same.
- **UI (read-only, D9):** grouped projects → tunnels (title, branch, path A→B) → running processes and their output. Mockup `V3-Compute`.

## 9. Files: dragging between machines

- When crossing the edge with a file drag in progress (Finder / Explorer), the source machine reads the dragged paths, `tandem-files` transfers them and the destination starts an equivalent drag with the files already copied into a temporary folder; on drop, the destination system does the normal copy.
- If the drag is cancelled, what was transferred is deleted. Progress in the UI (notice with a bar).
- It requires crossing with the button pressed: this feature enables crossing during a file drag even if "Drag between machines" is off for everything else.

## 10. Monitors (single standard)

- **Standard:** the protocol of the 34"'s own firmware (MCCS + VCP `0xEA`–`0xEF` events and verdicts, `0xEE` live state, `0x60` with the real input). A monitor is **supported** if the probe (`Get 0xEA`, reads only) confirms it. Any other: **not supported** (shown, not controlled). Vendor codes are never sent without a prior probe.
- **`monitor-ddc` crate** (repo `monitor-firmware`, A5): `DdcBus` trait (Get/Set VCP, capabilities, EDID), Windows (`dxva2`) and macOS (`IOAVService` on Apple Silicon) backends, a test double with personalities (standard, factory, disconnected), probe, event reading with a cursor, input change with verdict.
- **Owner per monitor (D13):** each agent reports which monitors it sees with a live DDC channel. The owner is the one with a channel; if both, the one with the lower response latency; if the owner leaves or loses the channel, the other is chosen. Only the owner reads events (the event cursor is global to the monitor).
- **Identity:** `<model>-<short hash of the EDID serial>`; it links the DP id and the HDMI id (the EDID changes per input).
- **Input changes:** a transaction with the firmware's verdict (`confirmed` / `failed`), timeout and rollback. In hardware tests, **automatic revert is mandatory**.
- **UI:** monitors with their real input, owner and events; input selector with a sliding indicator; unsupported ones in their own card. Mockup `V3-Monitors`.

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

- Unsigned installers: `.dmg` (macOS) and NSIS (Windows), on GitHub Releases when a `v*` tag is pushed.
- **Automatic update:** `tauri-plugin-updater` with the updater's own signature (minisign; not code signing) against GitHub Releases; it updates the UI and the agent; the two machines update one at a time without cutting the keyboard.

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
