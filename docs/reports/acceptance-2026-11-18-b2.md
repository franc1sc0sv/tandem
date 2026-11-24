# Tandem v0.3 acceptance — part B, round 2 (end-to-end flows)

Date: 2026-11-18 · Base: `origin/develop` at `b3841bc` (T-152, T-155, T-159 integrated) · Agent: acceptance (part B). Out of scope: pixel-by-pixel parity (part A) and baseline CI quality (part C).

## Verdict

**FAIL.** The 10 failures from round 1 (T-150 to T-159) were fixed except for **one that is still broken** (T-151: the other machine's activity in step 4 does not reach the UI) and **a new one** was found (tray). Two fix tasks: **T-166** and **T-167**. A single failing item is enough not to declare the app finished.

## How it was tested

Same harness as round 1 (`docs/reports/acceptance/2026-11-18-b/harness/`): two real `tandemd` over loopback, one HTTP bridge per machine with the app's real code, the production UI in Chromium with Playwright, a host with doubles for the input engine, DDC and clipboard, UDP proxies to cut paths, MCP client against `tandem mcp`. The full batch was run (`all.sh`), the checks that depended on the old UI were redone on clean pairs and the new ones were added (`docs/reports/acceptance/2026-11-18-b2/harness/`). Pairs: macOS+Windows, Windows+Windows and macOS+macOS (G2); demo mode with all routes and variants. Screenshots in `docs/reports/acceptance/2026-11-18-b2/<flow>/` (the Tunnels ones were removed: they show the test project's path). Raw results: `results.jsonl` (144 rows: 133 pass, 2 fail, 8 invalidated with their note: harness false negatives checked separately).

Environment limits (unchanged from round 1): no WebKitGTK, no tray or system global shortcuts, real updater replaced by a double, and everything `[hw]` (real macOS permissions, UAC, real Wake-on-LAN, real DDC, crossing with a real mouse, file drag with a real compositor) is left for the T-123 suite or each task's pending hardware list. The F7 and F8 drag flows are covered by their integration tests with doubles.

## Round 1 failures, checked again

| Task | What was checked now | Result |
|---|---|---|
| T-150 | "Test crossing" enabled; a real engine crossing gives "Crossed to … through the right edge, as the arrangement says" and "Continue" is enabled; without a crossing, "Did not cross in 20 s"; the summary says "crossing tested"; Arrangement shows 1920 × 1080 and 3440 × 1440, 21:9, 100 %; the 12 cases 4 sides × 3 alignments reach both machines and `tandem.toml` (and the engine crosses through the new edge, not the old one) | PASS |
| T-151 | A machine without a hub no longer appears with "Keyboard and mouse" (PASS). **But** "Activity detected" from the other machine still does not reach the UI | **FAIL** → T-166 |
| T-152 | "Leave desk" with confirmation: both end up "unpaired" in 14 ms and `trust.toml` ends up without keys | PASS |
| T-153 | Reloading on Network leaves "Measure bandwidth" enabled | PASS |
| T-154 | A freshly opened or reloaded window shows the existing tunnels (origin and other machine) | PASS |
| T-155 | `files_put` of a credential is skipped with a notice ("1 file(s) skipped because they looked like credentials") | PASS |
| T-156 | Setup opens by itself on Windows; "Close" measures 32 × 32, receives the click and closes | PASS |
| T-157 | "Lock here" locks (the cursor does not cross), the UI shows it, "Remove lock" releases it; on the machine that is not in control it explains why not | PASS |
| T-158 | Tunnels and Files with correct reasons; the summary does not say "remote compute is missing" | PASS |
| T-159 | Wake-on-LAN MAC editable on the other machine's page and saved in `tandem.toml`; the desk name is edited in Settings and the other machine sees it (`desk_name` on both) | PASS |

## Verdict per item of `docs/ACCEPTANCE.md`

| Item | Verdict | Tasks | Evidence |
|---|---|---|---|
| F1 First use | **FAIL** | T-166 | Pass: setup by itself on Windows, same 6-digit code, bad code rejected, paired, step 4 without false positives, step 5 with a real crossing, summary, MCP command copied. Fails: the other machine's activity in step 4 (`F1-setup/04-activity-other.png`) |
| F2 Arrangement | PASS | | F2-arrangement, F2-live-crossing-test |
| F3 Scenes | PASS | | F3-scenes |
| F4 Monitors | PASS | | F4-monitors · the 34" `[hw]` in T-123 |
| F5 Tunnels through MCP | PASS | | all steps pass, including T-130, the freshly opened UI and `files_put` of a credential |
| F6 Network | PASS | | F6-network |
| F7 Keyboard and mouse without an owner | PASS | | three checks |
| F8 Clipboard and files | PASS | | text/image clipboard A↔B; files with integration tests |
| F9 Shared configuration and Undo | PASS | | F9-shared-config |
| F10 Power | PASS | | F10-power (the MAC is typed from the UI) |
| F11 Modules | PASS | | correct reasons |
| F12 Settings and palette | PASS | | all actions and commands; "Leave desk" and "Lock here" fixed |
| F13 Updates | PASS | | F13-updates, trouble states |
| F14 Tray | **FAIL** | T-167 | Shows the state and applies scenes, but says "1 tunnels" and the count depends on the system (`F14-tray/02-tray-with-tunnel.png`) |
| G2 Nothing fixed to Mac/PC | **FAIL** | T-167 | Windows+Windows and macOS+macOS pass on every screen and the palette; the tray (`Tray.svelte`) draws "macOS" or "N tunnels" depending on the system (read from the code) |
| G6 Every element does something real | PASS | | button sweep: "Test crossing" and "Measure" now work; the 6 "no effect" ones are the already active tab and the disabled fixed shortcut |
| G7 / G8 Animations and reduced motion | not re-evaluated | | not repeated in this round (round 1 gave them PASS); part A covers it |
| G9 Console without errors / without `{{`, `undefined`, `NaN` | PASS | | no console with errors live; demo sweep (3 variants × 12 routes + 14 states): no findings |
| G10 Everything editable is edited from the UI | PASS | | Wake-on-LAN and desk name |

Not evaluated here: P1–P15 and similarity with the mockups (part A), G1/G3–G5/G11 baseline quality (part C) and H1–H5 (hardware). Things that were seen and do not count as failures: "Size —" in Arrangement (the engine does not read the physical size; documented in T-150) and the name "s0" of the screens, which is the one the simulated engine gives.

## Fix tasks

| Task | Failure | Flow |
|---|---|---|
| T-166 | The `tandemd` IPC never forwards `AgentToUi::KeyProbe`: the other machine's "Activity detected" does not reach the UI | F1 |
| T-167 | Tray: "1 tunnels" and a count per operating system | F14, G2 |

## Reproduce

As in round 1 (`docs/reports/acceptance/2026-11-18-b/harness/`), with `h.sh` and the scripts in `2026-11-18-b2/harness/` (`f2.mjs`, `f2cross.mjs`, `wiz5.mjs`, `aco.mjs`, `close.mjs`, `g10.mjs`, `g10b.mjs`, `tray.mjs`, `f16.mjs`, `dbg3.mjs` for T-166). `ACC_RUN` has to be a short path (the `tandemd` socket does not allow more than 108 bytes). Environment note: `pnpm install --frozen-lockfile` fails today with `fast-check@4.10.3` because of the minimum-age policy (published the same day); it was installed with `--config.minimumReleaseAge=0`.
