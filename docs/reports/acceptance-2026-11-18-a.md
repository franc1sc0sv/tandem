# Tandem v0.3 acceptance, part A (2026-11-18)

Part A of 3: visual parity and animations of every screen. Base: `origin/develop` at `04f2967`, detached worktree. Out of scope: part B (flows with real agents, F1-F14, `F-ClaudeMCP`) and part C (baseline quality, CI, G10, G11).

**Result: `ACCEPTANCE: FAIL 8`** (tasks T-140 to T-147). This part does not declare the app finished.

## How it was measured

- `pnpm shots --check` (66 screenshots and style parity of 22 components) on `dist-demo`, with the ACCEPTANCE thresholds: similarity ≥ 0.99 and worst tile ≥ 0.97. Full output: [`pnpm-shots.log`](acceptance/2026-11-18-a/pnpm-shots.log). The command exits 0; I re-applied the thresholds to each measurement (screen and zones) in `similarity.json` and only the five screenshots that the harness measures without a threshold stay below (table below).
- `pnpm mockups:shot` again: the 72 screenshots in `docs/mockups-png/` come out identical to the committed ones (references are current).
- Temporary (uncommitted) screenshots of the mockup states that `shots-spec.ts` does not measure: see T-144 and T-145.
- Masks: for each `ignore` and `content.masks` in `shots-spec.ts` I read what lies underneath in the mockup, its area and its reason: [`masks.txt`](acceptance/2026-11-18-a/masks.txt).
- Animations: in each of the 66 screenshots (mockup and app, with the same clicks) the computed animations and transitions were collected (name, duration, iterations, curve, `@keyframes` properties) plus the running ones (`document.getAnimations()` on every frame); the app without `still`. Comparison by signature: [`missing-animations.txt`](acceptance/2026-11-18-a/missing-animations.txt). With `prefers-reduced-motion: reduce` it was repeated on all 66: zero animations, zero transitions, zero running animations, zero console errors.
- Visual rules: 66 demo cases (the three pairs mac-win, win-win and mac-mac on the eleven routes, the four `trouble` states, the four Settings tabs, the six setup steps, progress in flight/success/failure, palette, notices, tray, tunnels empty and with 12 processes) with a dump of text, buttons, chips, lines on cards, fields, scales and errors; keyboard tabbing (visible focus); and 187 button clicks to detect the ones that change nothing.
- Evidence of the failures: `docs/reports/acceptance/2026-11-18-a/<case>/` (app screenshot, and mockup and diff when there is a mockup).

## Global rules

| Item | Result | Detail |
|---|---|---|
| G1 Only two machines | FAIL | "Add machine" / "Add another machine" on the "First time" card of States (T-142). No third machine in the sidebar, Desk, Settings, Palette or Setup. |
| G2 Nothing fixed to Mac/PC | FAIL | With two Windows machines the tunnels say "PC → PC" (T-141). Names, icons, colors and scaled desktops come from each machine in win-win and mac-mac (px per inch equal in each scene: 9.6 and 8.96). |
| G3 Sidebar | PASS | Desk, Tunnels with counter, Monitors, Arrangement, Network, one entry per machine and Settings on the 11 routes and in the three pairs. |
| G4 Buttons, icons, chips, cards | PASS | Every `.btn` has an icon; visible focus on 113 tabbed elements without an exception; no card with a colored line on top; uniform status chips (25 px, 12 px, pill; the other heights are the mockups'); style parity of 22 components ✓. List rows, scenes and combos without an icon are the mockups'. |
| G5 No filler, no "Keyboard here" | FAIL | "Wi-Fi · no cable" (T-147). There is no "Keyboard here", "requires cable", old modes or X/Y in the arrangement. |
| G6 Everything does something (part A) | PASS | 187 clicks: no button without an effect except system actions (open folder, System Settings, Quit) and those already selected; the real effect on the agent belongs to part B. |
| G7 Animations | FAIL | In the demo the Desk cursor restarts every 1.5 s and neither crosses nor flashes (T-140). The rest of the mockups' `@keyframes` and `transition` have an equivalent (minor duration differences, noted in `missing-animations.txt`). |
| G8 `prefers-reduced-motion` | PASS | Zero animations, transitions or running animations in the 66 screenshots. |
| G9 Console and text | PASS | Zero console errors or warnings in 66 cases; no `{{`, `undefined`, `NaN`, `null` or visible text in Spanish. |
| G10 Shared configuration | PART B | |
| G11 Baseline quality | PART C | |

## Screens

| # | Screen | Mockups and states | Result | Task |
|---|---|---|---|---|
| P1 | Desk | `V3-Desk` 99.99 % / 98.4 %; with scene Only MacBook and Gaming 99.99 % / 98.4 % (measured separately) | PASS in pixels; animation FAIL | T-140, T-144 |
| P2 | Tunnels | the 7 mockup states 100 % / 100 %; empty and 12 processes without comparison (same components) | PASS (labels of the win-win pair FAIL) | T-141 |
| P3 | Machine | `V3-Machine` 100 % / 99.5 % (content), `F-MachineMac` 99.99 % / 98.4 % | PASS (weak mask) | T-143 |
| P4 | Monitors and scenes | `V3-Monitors` ×2, `F-NewScene` ×3: 100 % / ≥ 98.9 % | PASS | |
| P5 | Arrangement | 4 sides × 3 alignments, chosen screen, unsupported: 100 % / ≥ 99.8 % | PASS | |
| P6 | Crossing test | waiting, crossed, timed out: 100 % / ≥ 99.7 %; wrong edge without a mockup (same components, checked in the DOM) | PASS | T-144 (ACCEPTANCE text) |
| P7 | Network | `V3-Network` 99.98 % / 97.9 % | PASS (Wi-Fi text FAIL) | T-147 |
| P8 | Setup | steps 1 to 6 and variants ≥ 99.98 % / ≥ 97.4 %; state "Create a new desk" 99.27 % / 83.0 % | FAIL | T-145 |
| P9 | Progress / power | 3 states: 99.88 % / 97.2 % | PASS | |
| P10 | Notices | 99.97 % / 99.4 % (only the stack; the rest is neither compared nor documented) | FAIL | T-146 |
| P11 | Settings | 5 tabs: ≥ 99.99 % / ≥ 99.2 % | PASS (README out of date) | T-146 |
| P12 | Palette | 99.95 % / 97.1 % with ≈ 16 % covered without a strong reason; "no results" 99.96 % / 97.1 %; "filter" does not represent a filter | PASS with reservations | T-143, T-144 |
| P13 | Trouble states | `V3-States` 100 % / 99.6 %; the four `trouble` states look right in Desk, Machine and Network | FAIL ("Add machine" text) | T-142 |
| P14 | Tray | 100 % / 100 % with 17.8 % covered without a strong reason | PASS with reservations | T-143 |
| P15 | What Claude sees | not an app screen; `tandem mcp` tools in part B | PART B | |

## Mask review

They have a strong reason and are in the README: "Add machine" (sidebar, Desk card, Settings row, palette), "Remove from desk", "Simulate…", "Stop waiting", "Send over Wi-Fi anyway", "Lock here", "Latency · 10 min", MCP sessions, the desk name as text, the other machine's pencil, the scene sentence with the real name, the `.cur` rectangles in `F-Setup5` and "1 missing". Without a strong reason: Palette, Tray, the "Open settings" button covered in `F-MachineMac` and the paragraph of `F-Setup5` (T-143). Mockup regions that nobody compares or documents, and an out-of-date README (Shortcuts): T-146.

## Screenshots (all)

The comparisons of this run; the result "INFORMATIVE" is a screenshot that the harness measures without a threshold because the mockup does not draw that state.

| Screenshot | Mockup / state | Similarity | Worst tile | Result |
|---|---|---|---|---|
| arrangement | V3-Arrangement | 100.00 % | 99.8 % | PASS |
| arrangement-bottom-center | V3-Arrangement--side-bottom-center | 100.00 % | 99.8 % | PASS |
| arrangement-bottom-end | V3-Arrangement--side-bottom-end | 100.00 % | 99.8 % | PASS |
| arrangement-bottom-start | V3-Arrangement--side-bottom-start | 100.00 % | 99.8 % | PASS |
| arrangement-top-center | V3-Arrangement--side-top-center | 100.00 % | 99.8 % | PASS |
| arrangement-top-end | V3-Arrangement--side-top-end | 100.00 % | 99.8 % | PASS |
| arrangement-top-start | V3-Arrangement--side-top-start | 100.00 % | 99.8 % | PASS |
| arrangement-right-center | V3-Arrangement--side-right-center | 100.00 % | 99.8 % | PASS |
| arrangement-right-end | V3-Arrangement--side-right-end | 100.00 % | 99.8 % | PASS |
| arrangement-right-start | V3-Arrangement--side-right-start | 100.00 % | 99.8 % | PASS |
| arrangement-left-center | V3-Arrangement--side-left-center | 100.00 % | 99.8 % | PASS |
| arrangement-left-end | V3-Arrangement--side-left-end | 100.00 % | 99.8 % | PASS |
| arrangement-left-start | V3-Arrangement--side-left-start | 100.00 % | 99.8 % | PASS |
| arrangement-unsupported | V3-Arrangement--screen-unsupported | 100.00 % | 99.8 % | PASS |
| arrangement-screen-mac | V3-Arrangement--screen-mac | 100.00 % | 99.8 % | PASS |
| settings | F-Settings | 100.00 % | 99.7 % | PASS |
| settings-updates | F-Settings | 100.00 % | 99.8 % | PASS |
| settings-updates-checked | F-Settings | 100.00 % | 99.8 % | PASS |
| settings-shortcuts | F-Settings | 100.00 % | 99.8 % | PASS |
| settings-advanced | F-Settings | 99.99 % | 99.2 % | PASS |
| settings-keyboard | F-Settings | 100.00 % | 99.8 % | PASS |
| setup-arrangement | F-Setup5 | 100.00 % | 100.0 % | PASS |
| setup-arrangement-crossed | F-Setup5--crossed | 100.00 % | 99.9 % | PASS |
| setup-arrangement-waiting | F-Setup5--waiting | 100.00 % | 100.0 % | PASS |
| setup-arrangement-other-edge | F-Setup5--other-edge | 99.99 % | 97.7 % | PASS |
| setup-ready | F-Setup6 | 100.00 % | 99.8 % | PASS |
| setup-machine | F-Setup1 | 99.99 % | 98.2 % | PASS |
| setup-machine-amber | F-Setup1--amber | 99.99 % | 98.2 % | PASS |
| setup-machine-teal | F-Setup1--teal | 99.99 % | 98.2 % | PASS |
| setup-machine-violet | F-Setup1--violet | 99.98 % | 97.4 % | PASS |
| setup-permissions | F-Setup3 | 99.98 % | 97.6 % | PASS |
| setup-permissions-all | F-Setup3--all-granted | 100.00 % | 100.0 % | PASS |
| setup-permissions-one | F-Setup3--one-granted | 99.99 % | 97.8 % | PASS |
| setup-desk | V3-Setup | 100.00 % | 99.8 % | PASS |
| setup-keyboard | F-Setup4 | 100.00 % | 100.0 % | PASS |
| notices | F-Notices | 99.97 % | 99.4 % | PASS |
| tray | V3-Tray | 100.00 % | 100.0 % | PASS |
| crossing | F-CrossingTest | 100.00 % | 100.0 % | PASS |
| crossing-crossed | F-CrossingTest--crossed | 100.00 % | 99.7 % | PASS |
| crossing-no-crossing | F-CrossingTest--no-crossing | 100.00 % | 99.7 % | PASS |
| crossing-other-edge | F-CrossingTest--no-crossing | 99.67 % | 84.6 % | INFORMATIVE: no threshold (the mockup does not draw it) |
| scene-shows-mac | F-NewScene--shows-mac | 100.00 % | 99.8 % | PASS |
| scene-new | F-NewScene | 100.00 % | 99.8 % | PASS |
| scene-unchanged | F-NewScene--unchanged | 100.00 % | 99.8 % | PASS |
| states | V3-States | 100.00 % | 99.6 % | PASS |
| machine-a | F-MachineMac | 99.99 % | 98.4 % | PASS |
| machine-b | V3-Machine | 100.00 % | 99.5 % | PASS |
| monitors | V3-Monitors | 100.00 % | 98.9 % | PASS |
| monitors-input-mac | V3-Monitors--input-mac | 100.00 % | 98.9 % | PASS |
| palette | F-Palette | 99.95 % | 97.1 % | PASS |
| progress-in-flight | F-Transactions | 99.88 % | 97.2 % | PASS |
| progress-success | F-Transactions--success | 99.88 % | 97.2 % | PASS |
| progress-rollback | F-Transactions--rollback | 99.88 % | 97.2 % | PASS |
| desk | V3-Desk | 99.99 % | 98.4 % | PASS |
| desk-mac-mac | V3-Desk | 98.26 % | 60.3 % | INFORMATIVE: no threshold (D2) |
| desk-win-win | V3-Desk | 96.83 % | 32.6 % | INFORMATIVE: no threshold (D2) |
| network | V3-Network | 99.98 % | 97.9 % | PASS |
| tunnels | V3-Compute | 100.00 % | 100.0 % | PASS |
| tunnels-closed | V3-Compute--tunnel-closed | 100.00 % | 100.0 % | PASS |
| tunnels-twelve-processes | V3-Compute | 99.25 % | 81.1 % | INFORMATIVE: no threshold (the mockup does not draw it) |
| tunnels-two-processes | V3-Compute--tunnel-2-processes | 100.00 % | 100.0 % | PASS |
| tunnels-lora | V3-Compute--tunnel-lora | 100.00 % | 100.0 % | PASS |
| tunnels-process-two | V3-Compute--process-2 | 100.00 % | 100.0 % | PASS |
| tunnels-project-collapsed | V3-Compute--project-collapsed | 100.00 % | 100.0 % | PASS |
| tunnels-no-processes | V3-Compute--tunnel-no-processes | 100.00 % | 100.0 % | PASS |
| tunnels-empty | V3-Compute | 98.18 % | 82.4 % | INFORMATIVE: no threshold (the mockup does not draw it) |

## Tasks created

| Task | Failure |
|---|---|
| T-140 | Desk cursor in the demo restarts every 1.5 s; no crossing or flash (G7, P1) |
| T-141 | "PC → PC" with two Windows machines in Tunnels (G2) |
| T-142 | "Add machine" in States (G1, P13) |
| T-143 | Masks without a strong reason: Palette, Tray, Machine, Setup |
| T-144 | Mockup states not measured; ACCEPTANCE.md contradicts the mockups in P6 |
| T-145 | Setup "Create a new desk" vs `V3-Setup--new` (99.27 % / 83.0 %) |
| T-146 | Notices: "System notifications" region neither compared nor documented; masks README out of date |
| T-147 | "Wi-Fi · no cable" (G5, P7) |

## Could not be checked here

- State `F-Notices--added` ("Add notice" is a mockup control; the app has no such button).
- The flows, the effect on the agents and `[hw]`: part B. `develop` CI and baseline quality: part C.
