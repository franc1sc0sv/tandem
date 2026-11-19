# Tandem v0.3 acceptance, part A, round 5 (2026-11-19)

Part A of 3: visual parity, animations and visible text of every screen, in the three simulated pairs (`mac-win`, `win-win`, `mac-mac`). Base: `origin/develop` at `95af721` (T-244), which includes T-243/T-245 (`442614e`) and T-250/T-251 (`cd83010`). `develop` later moved to `985403a` (part C report and T-290), docs only, so the results hold for it. `scripts/ci/status.sh` fails on `develop` before this PR (STATUS.md drift, T-290 from part C); this PR does not touch `STATUS.md`, T-290 counts T-270 when it refreshes it. Out of scope: part B (flows with real agents) and part C (baseline quality, CI, G10, G11).

**Result: `ACCEPTANCE: FAIL 1`** (task T-270). Every screen and state is at parity, motion and reduced motion pass, and the round 4 findings T-243, T-244 and T-245 are fixed. One new failure comes from the T-244 fix itself: the "Allow remote execution" switch on the other machine's page is now locked, so it looks like a working switch and does nothing (G6), and the decision behind it is not recorded.

## Round 4 failures, re-checked

| Task | Finding | Result | Evidence |
|---|---|---|---|
| [T-243](../tasks/T-243.md) | Network notices asked to edit `tandem.toml`, no icon, lower-case message | PASS | Injected with the exact texts of `link_notices` after the fix (one Ethernet, one "Several addresses…" Wi-Fi, the link manager). Each notice is `icon + text` with the tray warning tokens (border `#5a4a2c`, background `#1e1a12`, text `#e6c48f`, radius 12 px, same as `V3-Tray`); messages start with a capital; no `tandem.toml` or `[network]` on any of the 279 cases. Screenshots: [`r5-network-notices`](acceptance/2026-11-19-a5/new-screens/r5-network-notices.png), [`win-win`](acceptance/2026-11-19-a5/new-screens/r5-network-notices-win-win.png), [`mac-mac`](acceptance/2026-11-19-a5/new-screens/r5-network-notices-mac-mac.png). |
| [T-244](../tasks/T-244.md) | Demo tied to Mac/PC in `win-win`/`mac-mac` | PASS, with a new failure ([T-270](../tasks/T-270.md)) | Seeded notices name only the desk: `win-win` "27" confirmed on PC-Workshop in 1.8 s." and "PC-Desktop → PC-Workshop · WSL."; `mac-mac` "Studio Display confirmed on Mac mini in 1.8 s." and "MacBook Pro → Mac mini · zsh."; `mac-win` unchanged (`notices` 99.97 % / 99.4 %). `admin=off` shows the macOS note on macOS machines (`mac-win` here=a, `mac-mac`) and the Windows note on Windows machines (`mac-win` here=b, `win-win`). The States reference view has no Permissions card and no "Open System Settings" on Windows machines (`mac-win` here=b, `win-win`), and keeps it on macOS. Only Keyboard and mouse and Clipboard are switches in the Modules list (35 cases); Monitors, Tunnels, Protected screens and Power are plain chips. The "Allow remote execution" row became a locked switch: see G6 and T-270. |
| [T-245](../tasks/T-245.md) | "Protected screens" line broken English | PASS | `modules.rs` (agent), `demo.ts` and `V3-Machine` all say "The mouse and keyboard also work on UAC prompts and at the sign-in screen."; the demo machine pages on Windows machines show it, macOS machines do not list the module. |
| [T-250](../tasks/T-250.md) | Save and apply skipped the progress screen | PASS (part A) | The race needs two real agents (part B). Here: `F-Transactions` parity unchanged (in progress, success, rollback 99.88 % / 97.9 %), and `src/screens/races.test.ts` passes in `pnpm test`. The new waiting line "Starting the change…" uses the screen's `mut` text, no new component. |
| [T-251](../tasks/T-251.md) | Update all lost its step | PASS (part A) | Same: the Updates tab is at parity (`settings-updates` and `settings-updates-checked` 100 % / 99.8 %); the race is part B and its tests pass. |

## How it was measured

- `pnpm shots --check` on `dist-demo`: 72 screenshots and the style parity of 22 components, thresholds ≥ 0.99 and worst tile ≥ 0.97. Exit 0, working tree clean afterwards (app screenshots byte-identical to the committed ones). Output: [`pnpm-shots.log`](acceptance/2026-11-19-a5/pnpm-shots.log).
- `pnpm mockups:shot` re-rendered the 72 mockup PNGs: byte-identical to `docs/mockups-png` (no mockup changed since round 4).
- Sweep ([`harness/sweep.ts`](acceptance/2026-11-19-a5/harness/sweep.ts), round 4 harness updated): 279 demo cases, the 261 of round 4 plus 18 for what changed (admin note per system and pair, States view per system, seeded notices per pair, machine pages per pair and in `peer-off`/`link-down`, Network notices in `mac-mac`). The injected Network notices now use the post-T-243 texts. New checks: every `role="switch"` with its state, `aria-readonly` and title; "System Settings" on a Windows machine; `tandem.toml` or `[network]` in visible text. Summary: [`sweep-summary.json`](acceptance/2026-11-19-a5/sweep-summary.json); distinct visible lines: [`visible-text.txt`](acceptance/2026-11-19-a5/visible-text.txt) (826).
- Locked switch ([`harness/switch.ts`](acceptance/2026-11-19-a5/harness/switch.ts), [`locked-switch/`](acceptance/2026-11-19-a5/locked-switch/)): attributes, computed style, then a click and Space, in 7 pair/trouble states.
- Animations ([`harness/motion.ts`](acceptance/2026-11-19-a5/harness/motion.ts), [`motion.txt`](acceptance/2026-11-19-a5/motion.txt)): byte-identical to round 4's output. The Desk cursor was sampled again with [`harness/probe.ts`](acceptance/2026-11-19-a5/harness/probe.ts) for 8 s.
- `pnpm test` (51 files, 733 tests) and `pnpm check` (523 files, 0 errors, 0 warnings) pass.

## Global rules

| Item | Result | Detail |
|---|---|---|
| G1 Only two machines | PASS | No "Add machine" or third machine in 279 cases. |
| G2 Nothing fixed to Mac/PC | PASS | All T-244 points fixed (above). The only Mac-word hits on `win-win` are `⌘K` in nine cases that the harness opens with a macOS user agent (as in round 4); with a Windows user agent every route shows `Ctrl+K`. |
| G3 Sidebar | PASS | On all main routes in the three pairs; sidebar regions 100 % on the compared shots, ≥ 99.56 % on the D2 desks. |
| G4 Buttons, icons, chips | PASS | Style parity of 22 components passes. Buttons without an SVG are the same cards, options, switches, radios and steps as in round 4, all drawn that way in the mockups. The Network notices now carry the warning icon. No colored top line on any card (0 in 279 cases). |
| G5 No filler | PASS | No "Keyboard here", "requires cable" or old modes. |
| G6 Everything does something | **FAIL** | [T-270](../tasks/T-270.md). "Allow remote execution" (other machine's page, all pairs, 26 cases) is `role="switch"`, on, `opacity: 1`, the same green track as the working switches, `aria-readonly`/`aria-disabled` true. A click and Space do nothing, with no toast and no visible explanation; only a hover tooltip, "Always on: remote execution can't be turned off from here." ("from here": the agent never lets Tunnels be switched anywhere). DECISIONS T-115 (a switch for the `tunnels` module) and T-098 (Tunnels has no switch) now disagree with no entry settling it. |
| G7 Animations | PASS | See below. |
| G8 Reduced motion | PASS | 0 running animations, 0 declared animations or transitions in all 279 cases. |
| G9 Console and text | PASS | 0 console errors or warnings and 0 external requests. One harness click timeout (`new:crossing-crossed`, no "Test crossing" button: the screen starts the test by itself), covered by the `crossing-crossed` shot. Observation, no task: the per-user administrator note copies the app core's text, "user-level installation: Tandem is not administrators-only. …", which is lower case like every setup row note in `F-Setup1`. |
| G10 Shared configuration | PART B | D4 on screen: no visible text asks to edit `tandem.toml`. Settings → Advanced shows the file's location with "Show file", as `F-Settings--advanced` draws it. |
| G11 Baseline quality | PART C | |

### G7 detail

`motion.txt` is byte-identical to round 4: 57 of 72 cases with 0 gaps, and the 15 others are the ones round 4 checked by hand (Desk `still` pipes and the JavaScript cursor, the crossing-test mockup-only buttons and merged waiting step, the duplicated `.go` class in `F-Setup5`, notices without a life bar while seeded, `tunnels-empty` not drawn by the mockup). Re-checked live: without `still` the pipes are `fl 1s` and `fl 1.3s` infinite linear, and the Desk cursor runs its 7 s loop (x 351 → 670 → 351) with the edge flashing on each crossing (opacity 0.887 at 1.5 s, 0.872 at 6.0 s).

## Screens

| # | Screen | Result | Note |
|---|---|---|---|
| P1 | Desk | PASS | 99.99 % / 98.4 %, with the two scenes too. |
| P2 | Tunnels | PASS | 7 mockup states at 100 % / 100 %; empty and 12 processes informative. |
| P3 | Machine | FAIL | Parity passes (`V3-Machine` 99.99 % / 98.6 %, `F-MachineMac` 99.99 % / 98.4 %), Protected screens text fixed; the locked switch fails G6 ([T-270](../tasks/T-270.md)). |
| P4 | Monitors and scenes | PASS | |
| P5 | Arrangement | PASS | 4 sides × 3 alignments, chosen screen, unsupported: 100 % / 99.7 %. |
| P6 | Crossing test | PASS | 100 % / ≥ 99.7 %. |
| P7 | Network | PASS | 99.98 % / 97.9 %; notices fixed (T-243). |
| P8 | Setup | PASS | Steps 1–6 and variants; admin note per system (T-244). |
| P9 | Progress / power | PASS | 99.88 % / 97.9 %. |
| P10 | Notices | PASS | `mac-win` 99.97 % / 99.4 %; other pairs name only their machines. |
| P11 | Settings | PASS | Five tabs at ≥ 99.99 % / ≥ 99.2 %. |
| P12 | Palette | PASS | ≥ 99.96 % / ≥ 97.0 %. |
| P13 | Trouble states | PASS | 100 % / 99.9 %; no Permissions card on Windows machines. |
| P14 | Tray | PASS | 99.93 % / 98.0 %. |
| P15 | What Claude sees | PART B | |

## Screenshots of the re-checked states

None of these is a mockup state; they are judged by components, tokens and meaning.

| State | Screenshot |
|---|---|
| Admin note, macOS (`mac-win` here=a, `mac-mac`) | [`r5-admin-off-macos`](acceptance/2026-11-19-a5/new-screens/r5-admin-off-macos.png), [`r5-admin-off-mac-mac`](acceptance/2026-11-19-a5/new-screens/r5-admin-off-mac-mac.png) |
| Admin note, Windows (`mac-win` here=b, `win-win`) | [`r5-admin-off-windows`](acceptance/2026-11-19-a5/new-screens/r5-admin-off-windows.png), [`r5-admin-off-win-win`](acceptance/2026-11-19-a5/new-screens/r5-admin-off-win-win.png) |
| States view per system | [`r5-states-macos`](acceptance/2026-11-19-a5/new-screens/r5-states-macos.png), [`r5-states-windows`](acceptance/2026-11-19-a5/new-screens/r5-states-windows.png), [`r5-states-win-win`](acceptance/2026-11-19-a5/new-screens/r5-states-win-win.png), [`r5-states-mac-mac`](acceptance/2026-11-19-a5/new-screens/r5-states-mac-mac.png) |
| Seeded notices per pair | [`r5-notices-win-win`](acceptance/2026-11-19-a5/new-screens/r5-notices-win-win.png), [`r5-notices-mac-mac`](acceptance/2026-11-19-a5/new-screens/r5-notices-mac-mac.png) |
| Machine pages | [`mac-win` b](acceptance/2026-11-19-a5/new-screens/r5-machine-b-mac-win.png), [from Windows](acceptance/2026-11-19-a5/new-screens/r5-machine-a-from-windows.png), [`win-win` a](acceptance/2026-11-19-a5/new-screens/r5-machine-a-win-win.png), [`win-win` b](acceptance/2026-11-19-a5/new-screens/r5-machine-b-win-win.png), [`mac-mac` b](acceptance/2026-11-19-a5/new-screens/r5-machine-b-mac-mac.png), [peer off](acceptance/2026-11-19-a5/new-screens/r5-machine-b-peer-off.png), [link down](acceptance/2026-11-19-a5/new-screens/r5-machine-b-link-down.png) |
| Locked switch (T-270), per pair and state, hovered | [`locked-switch/`](acceptance/2026-11-19-a5/locked-switch/); mockup: `docs/mockups-png/V3-Machine.png` |

## Screenshots (all)

"INFORMATIVE" marks a state the mockup does not draw (accepted by the ACCEPTANCE rules, no pixel comparison).

| Screenshot | Mockup / state | Similarity | Worst tile | Result |
|---|---|---|---|---|
| desk | V3-Desk | 99.99 % | 98.4 % | PASS |
| desk-scene-macbook-only | V3-Desk--scene-macbook-only | 99.99 % | 98.4 % | PASS |
| desk-scene-game | V3-Desk--scene-game | 99.99 % | 98.4 % | PASS |
| desk-win-win | V3-Desk | 96.79 % | 32.6 % | INFORMATIVE (D2 desks differ by design) |
| desk-mac-mac | V3-Desk | 98.28 % | 60.3 % | INFORMATIVE (D2 desks differ by design) |
| tunnels | V3-Compute | 100.00 % | 100.0 % | PASS |
| tunnels-process-two | V3-Compute--process-2 | 100.00 % | 100.0 % | PASS |
| tunnels-two-processes | V3-Compute--tunnel-2-processes | 100.00 % | 100.0 % | PASS |
| tunnels-no-processes | V3-Compute--tunnel-no-processes | 100.00 % | 100.0 % | PASS |
| tunnels-lora | V3-Compute--tunnel-lora | 100.00 % | 100.0 % | PASS |
| tunnels-closed | V3-Compute--tunnel-closed | 100.00 % | 100.0 % | PASS |
| tunnels-project-collapsed | V3-Compute--project-collapsed | 100.00 % | 100.0 % | PASS |
| tunnels-empty | V3-Compute | 98.26 % | 82.4 % | INFORMATIVE |
| tunnels-twelve-processes | V3-Compute | 99.24 % | 81.1 % | INFORMATIVE |
| monitors | V3-Monitors | 100.00 % | 98.9 % | PASS |
| monitors-input-mac | V3-Monitors--input-mac | 100.00 % | 98.9 % | PASS |
| scene-new | F-NewScene | 100.00 % | 99.7 % | PASS |
| scene-shows-mac | F-NewScene--shows-mac | 100.00 % | 99.7 % | PASS |
| scene-unchanged | F-NewScene--unchanged | 100.00 % | 99.7 % | PASS |
| progress-in-progress | F-Transactions | 99.88 % | 97.9 % | PASS |
| progress-success | F-Transactions--success | 99.88 % | 97.9 % | PASS |
| progress-rollback | F-Transactions--rollback | 99.88 % | 97.9 % | PASS |
| arrangement | V3-Arrangement | 100.00 % | 99.7 % | PASS |
| arrangement-top-start | V3-Arrangement--side-top-start | 100.00 % | 99.7 % | PASS |
| arrangement-top-center | V3-Arrangement--side-top-center | 100.00 % | 99.7 % | PASS |
| arrangement-top-end | V3-Arrangement--side-top-end | 100.00 % | 99.7 % | PASS |
| arrangement-bottom-start | V3-Arrangement--side-bottom-start | 100.00 % | 99.7 % | PASS |
| arrangement-bottom-center | V3-Arrangement--side-bottom-center | 100.00 % | 99.7 % | PASS |
| arrangement-bottom-end | V3-Arrangement--side-bottom-end | 100.00 % | 99.7 % | PASS |
| arrangement-left-start | V3-Arrangement--side-left-start | 100.00 % | 99.7 % | PASS |
| arrangement-left-center | V3-Arrangement--side-left-center | 100.00 % | 99.7 % | PASS |
| arrangement-left-end | V3-Arrangement--side-left-end | 100.00 % | 99.7 % | PASS |
| arrangement-right-start | V3-Arrangement--side-right-start | 100.00 % | 99.7 % | PASS |
| arrangement-right-center | V3-Arrangement--side-right-center | 100.00 % | 99.7 % | PASS |
| arrangement-right-end | V3-Arrangement--side-right-end | 100.00 % | 99.7 % | PASS |
| arrangement-screen-mac | V3-Arrangement--screen-mac | 100.00 % | 99.7 % | PASS |
| arrangement-unsupported | V3-Arrangement--screen-unsupported | 100.00 % | 99.7 % | PASS |
| crossing | F-CrossingTest | 100.00 % | 100.0 % | PASS |
| crossing-crossed | F-CrossingTest--crossed | 100.00 % | 99.7 % | PASS |
| crossing-no-cross | F-CrossingTest--no-cross | 100.00 % | 99.7 % | PASS |
| crossing-other-edge | F-CrossingTest--no-cross | 99.65 % | 84.7 % | INFORMATIVE |
| setup-machine | F-Setup1 | 99.99 % | 98.2 % | PASS |
| setup-machine-violet | F-Setup1--violet | 99.98 % | 97.4 % | PASS |
| setup-machine-teal | F-Setup1--teal | 99.99 % | 98.2 % | PASS |
| setup-machine-amber | F-Setup1--amber | 99.99 % | 98.2 % | PASS |
| setup-desk | V3-Setup | 100.00 % | 99.6 % | PASS |
| setup-desk-new | V3-Setup--new | 100.00 % | 99.6 % | PASS |
| setup-permissions | F-Setup3 | 100.00 % | 99.6 % | PASS |
| setup-permissions-one | F-Setup3--one-granted | 100.00 % | 99.6 % | PASS |
| setup-permissions-all | F-Setup3--all-granted | 100.00 % | 99.6 % | PASS |
| setup-keyboard | F-Setup4 | 100.00 % | 99.6 % | PASS |
| setup-arrangement | F-Setup5 | 100.00 % | 99.6 % | PASS |
| setup-arrangement-waiting | F-Setup5--waiting | 100.00 % | 99.6 % | PASS |
| setup-arrangement-crossed | F-Setup5--crossed | 100.00 % | 99.6 % | PASS |
| setup-arrangement-other-edge | F-Setup5--other-edge | 100.00 % | 99.6 % | PASS |
| setup-done | F-Setup6 | 100.00 % | 99.6 % | PASS |
| network | V3-Network | 99.98 % | 97.9 % | PASS |
| machine-b | V3-Machine | 99.99 % | 98.6 % | PASS |
| machine-a | F-MachineMac | 99.99 % | 98.4 % | PASS |
| settings | F-Settings | 100.00 % | 99.7 % | PASS |
| settings-shortcuts | F-Settings--shortcuts | 100.00 % | 99.8 % | PASS |
| settings-keyboard | F-Settings--keyboard | 100.00 % | 99.8 % | PASS |
| settings-updates | F-Settings--updates | 100.00 % | 99.8 % | PASS |
| settings-updates-checked | F-Settings--updates-checked | 100.00 % | 99.8 % | PASS |
| settings-advanced | F-Settings--advanced | 99.99 % | 99.2 % | PASS |
| palette | F-Palette | 99.96 % | 97.8 % | PASS |
| palette-filter | F-Palette--filter | 99.97 % | 97.8 % | PASS |
| palette-no-results | F-Palette--no-results | 99.96 % | 97.0 % | PASS |
| states | V3-States | 100.00 % | 99.9 % | PASS |
| notices | F-Notices | 99.97 % | 99.4 % | PASS |
| notices-closed | F-Notices--closed | 99.98 % | 99.4 % | PASS |
| tray | V3-Tray | 99.93 % | 98.0 % | PASS |

## Failures and tasks

| Task | Failure |
|---|---|
| [T-270](../tasks/T-270.md) | "Allow remote execution" is a locked switch that looks live and does nothing (G6); DECISIONS T-115 and T-098 disagree on it and the T-244 choice is not recorded. |

## Reproducing

```sh
cd apps/tandem-ui && pnpm install && pnpm shots --check          # builds dist-demo
cd ../../tools/mockup-render && pnpm install && pnpm mockups:shot  # mockup references
cd ../.. && node docs/reports/acceptance/2026-11-19-a5/harness/sweep.ts <out>
node docs/reports/acceptance/2026-11-19-a5/harness/motion.ts <out>
node docs/reports/acceptance/2026-11-19-a5/harness/switch.ts <out>
node docs/reports/acceptance/2026-11-19-a5/harness/probe.ts 'demo' '#/desk' '' 8000
```
