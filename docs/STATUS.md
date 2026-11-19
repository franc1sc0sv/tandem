# Tandem — Project status

> **v0.3** cut: 2026-11-18 · `develop` branch, status as of 2026-11-19 (after T-291) · `develop` CI passes on Linux, Windows and macOS. The rare macOS flake of `pairing_hot_opens_the_input_session` (T-190) is explained and fixed by T-291.
> Design in `DESIGN.md` (v0.3) and `DECISIONS.md`; definition of done in `ACCEPTANCE.md`; autonomous work in `dev/night-run.md`; acceptance reports in `reports/`.

## In one line

**Everything that can be built and tested without your hardware is done; what remains is testing it on your real Mac, PC and monitors** (the `scripts/hw/run.sh` suite, T-123, is ready to run) and closing the failures found by the acceptance run. The manual runs over the Thunderbolt cable (`reports/hw-2026-11-18-cable.md` and a second run on 2026-11-19) already passed pairing, tunnels, monitor input switching and Mac-side input latency, and surfaced and closed several real bugs (T-196, T-197, T-198, T-237…T-241); an overnight soak that lost the link (cause unknown; T-242 now keeps the logs) still has to be re-run.

| | |
|---|---|
| Tasks | **173 done of 177** (`T-001`…`T-291`; open ones below) |
| Tests | **1623 Rust + 732 UI (vitest)**, 0 failures, 11 ignored (`#[ignore = "hardware"]` and latency benches) |
| Quality | `fmt`, `clippy -D warnings`, `svelte-check` and `pnpm build` with no warnings |
| CI per PR | Linux · Windows · macOS · UI x3 · coverage · `.dmg` · Windows installer |

## What exists

| Area | Status |
|---|---|
| **Machines** | Two symmetric machines, `Machine::{A, B}` (A1), with their system, name and color in the config (`MachineInfo`); there is no fixed "Mac" and "PC" (D2) |
| **Network** | Encrypted QUIC with mutual authentication; USB4 → Ethernet → Wi-Fi links with failover and hysteresis. **Data can also go over Wi-Fi** (A2): tunnels and files on low-priority, capped streams, so they add no latency to input. mDNS and pairing with a 6-digit code |
| **`tandemd` agent** | Orchestrator for scenes, power, input, clipboard, tunnels, files and roles; tested with two real agents in `tandem-sim` |
| **Scenes** | Replace modes (D6): a scene stores what each monitor shows and the arrangement, and is applied as a transaction with rollback |
| **Monitors** | A single standard (D11): the monitor that speaks the own-firmware protocol is controlled; otherwise "unsupported" appears. EDID reading and per-input cables (`[[cables]]`) |
| **Keyboard and mouse** | Own engine, no owner (D7): they work from either machine and Tandem follows the one in use (arbitration through messages between agents). Live cursor with subscription (up to 30 Hz) |
| **Clipboard, files, tunnels** | Text, HTML and image without echo; file drag; port tunnels |
| **Power** | Wake-on-LAN, clean power off, power-loss detection |
| **UI** | Working mockup (T-115) in real and demo mode, menu bar, tray, shortcuts, parity screenshots in `docs/screenshots/v03/` |
| **Hardware tests** | `crates/tandem-hw` suite + `scripts/hw/run.sh` (T-123); tests that change a monitor's input are opt-in (`--with-monitors`) and roll back automatically (A8) |
| **Installation** | macOS `.dmg` with LaunchAgent; Windows installer; CI release on a `v*` tag (draft) |

## Open tasks

| Task | Status | What |
|---|---|---|
| T-088, T-095, T-096 | blocked | Ignored monitors, own firmware for the 34" and live UI |
| T-124 | todo | End-to-end acceptance |
| T-140…T-165, T-190…T-291 | done | Failures from the acceptance and hardware runs, personal data out of the repo, CI time limits, English migration (T-200…T-208), one-way links (T-237), macOS and Windows input fixes (T-239, T-241), UI reply races (T-250, T-251), monitor report race after pairing (T-280), this file brought up to date (T-290), a rejection right after pairing no longer unpairs (T-291, supersedes T-190) |

## Pending hardware verification

Each task with hardware has its list in `docs/tasks/T-xxx.md`, section `## Pending hardware verification`. Tasks with open items:

- **Keyboard and mouse:** T-022, T-023, T-024, T-025, T-074, T-093 (capture and injection on macOS/Windows, ES layout, scroll, lock).
- **Screens and monitors:** T-010, T-011, T-012, T-013, T-014, T-015, T-104, T-109, T-110, T-111, T-121 (CGS/CCD on the Mac, DDC/CI, inputs with automatic rollback).
- **Network and power:** T-006, T-030, T-070, T-071, T-078, T-080, T-081, T-082 (mDNS, firewall, cables, Wake-on-LAN, installers).
- **Roles, clipboard, UI:** T-028, T-029, T-087, T-089, T-090, T-092, T-094, T-105, T-108, T-122, T-131.
- **Full suite:** T-123 (`scripts/hw/run.sh` end to end, then tick the covered items).

## Known risks and limitations

- **Hardware testing is under way, not finished.** Two real-cable runs between the Mac and the PC (the first in `reports/hw-2026-11-18-cable.md`, the second on 2026-11-19 at `develop` facf263, after T-196, T-197 and T-237) have found and closed real bugs. What passed on real hardware: pairing over USB4, ping (p50 and p99 of 1 ms), and the process, file, port and workspace tunnels in both directions; the Mac's `input-latency` check (PC to Mac input, p50 251 µs, p99 735 µs over 307 events, with Accessibility granted); and the monitor input round-trip and watchdog checks (2026-11-18, 11.6 s and 14.5 s). What failed: the PC's `input-cross`, because Windows capture ignored injected motion (fixed by T-239, to be re-verified). **Open: an overnight soak lost the link after about 45 minutes and never recovered; the evidence was lost (T-242 now keeps the logs), the cause is unknown and the soak must be re-run.** Wake-on-LAN, the SYSTEM service and the UI have not been exercised on real hardware. ssh to the Mac is disabled by company policy (T-198), so the Mac half is run by hand.
- **Private macOS APIs** (`SetsCursorInBackground`, `CGSConfigureDisplayEnabled`): they may change between versions.
- **Signing:** macOS with ad-hoc signing (TCC permissions may be requested again on reinstall); Windows without Authenticode (SmartScreen warns).
- **Windows SYSTEM service (T-027):** implemented and tested with doubles, but never installed on the real PC (its checklist is pending), and the installer is not Authenticode-signed.
- **Power loss during a change that uses the other machine:** waits up to ~130 s before marking it off (T-063).
- **Monitors:** the input switch depends on the 34" own firmware (T-095, blocked); tests that change inputs require automatic rollback because the user can be left without seeing a machine.

## What is left before going to `main`

Close the open acceptance failures, run the hardware suite (T-123) and review `develop`; the PR to `main` is opened by the user.
