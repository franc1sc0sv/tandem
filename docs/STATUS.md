# Tandem — Project status

> **v0.3** cut: 2026-11-18 · `develop` branch (cut after `3df14cb`) · `develop` CI passes on Linux, Windows and macOS except for known flaky tests (T-160, T-190).
> Design in `DESIGN.md` (v0.3) and `DECISIONS.md`; definition of done in `ACCEPTANCE.md`; autonomous work in `dev/night-run.md`; acceptance reports in `reports/`.

## In one line

**Everything that can be built and tested without your hardware is done; what remains is testing it on your real Mac, PC and monitors** (the `scripts/hw/run.sh` suite, T-123, is ready to run) and closing the failures found by today's acceptance run.

| | |
|---|---|
| Tasks | **119 done of 125** (`T-001`…`T-193`; open ones below) |
| Tests | **1442 Rust + 619 UI (vitest)**, 0 failures, 11 ignored (`#[ignore = "hardware"]` and latency benches) |
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
| T-027 | todo (`windows-hw`, `user`) | Windows SYSTEM service for UAC; needs a decision on the Authenticode certificate |
| T-088, T-095, T-096 | blocked | Ignored monitors, own firmware for the 34" and live UI |
| T-124 | todo | End-to-end acceptance |
| T-190 | todo | Persistent failure of `pairing_hot_opens_the_input_session` on macOS |
| T-140…T-165, T-191…T-193 | done | Failures from parts A, B and C of the acceptance run, personal data out of the repo, CI time limits, up-to-date documentation and CI checks |

## Pending hardware verification

Each task with hardware has its list in `docs/tasks/T-xxx.md`, section `## Pending hardware verification`. Tasks with open items:

- **Keyboard and mouse:** T-022, T-023, T-024, T-025, T-074, T-093 (capture and injection on macOS/Windows, ES layout, scroll, lock).
- **Screens and monitors:** T-010, T-011, T-012, T-013, T-014, T-015, T-104, T-109, T-110, T-111, T-121 (CGS/CCD on the Mac, DDC/CI, inputs with automatic rollback).
- **Network and power:** T-006, T-030, T-070, T-071, T-078, T-080, T-081, T-082 (mDNS, firewall, cables, Wake-on-LAN, installers).
- **Roles, clipboard, UI:** T-028, T-029, T-087, T-089, T-090, T-092, T-094, T-105, T-108, T-122, T-131.
- **Full suite:** T-123 (`scripts/hw/run.sh` end to end, then tick the covered items).

## Known risks and limitations

- **Almost nothing has been tested on real hardware yet.** Everything passes with test doubles, the desk simulator and CI; the physical side (macOS APIs, Windows hooks, DDC/CI, keyboard layout) can only be confirmed on your machines.
- **Flaky tests in CI** (macOS and Ubuntu; T-160 did not fully close them, see T-190 and T-191): not regressions, but they turn CI red at random or hang.
- **Private macOS APIs** (`SetsCursorInBackground`, `CGSConfigureDisplayEnabled`): they may change between versions.
- **Signing:** macOS with ad-hoc signing (TCC permissions may be requested again on reinstall); Windows without Authenticode (SmartScreen warns).
- **Windows without the SYSTEM service (T-027):** the remote mouse does not work at UAC or at the sign-in screen.
- **Power loss during a change that uses the other machine:** waits up to ~130 s before marking it off (T-063).
- **Monitors:** the input switch depends on the 34" own firmware (T-095, blocked); tests that change inputs require automatic rollback because the user can be left without seeing a machine.

## What is left before going to `main`

Close the open acceptance failures, run the hardware suite (T-123) and review `develop`; the PR to `main` is opened by the user.
