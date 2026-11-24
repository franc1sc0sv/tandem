# Tandem v0.3 acceptance, part A, round 6 (2026-11-19)

Part A of 3: visual parity, animations and visible text of every screen, in the three simulated pairs (`mac-win`, `win-win`, `mac-mac`). Base: `origin/develop` at `9fe53be` (T-291), which includes T-270 (`8cda900`), T-280 and T-290. `develop` later moved to `80366c3` (part C report, T-320, T-321): docs and the stray root `package.json` only, nothing under `apps/`, `tools/` or `mockups/`, so the results hold for it; this branch starts from `80366c3`. Out of scope: part B (flows with real agents) and part C (baseline quality, CI, G10, G11).

**Result: `ACCEPTANCE: FAIL 1`** (task T-300). Every screen and state is at parity with no new masks, motion and reduced motion pass, the 279-case sweep of the three pairs is clean, and T-270 is fixed in the app. One leftover of T-270: `V3-Machine.dc.html` still declares the switch styles (and their two transitions) of the row it removed, with no element using them.

## Round 5 failure, re-checked

| Task | Finding | Result | Evidence |
|---|---|---|---|
| [T-270](../tasks/T-270.md) | "Allow remote execution" was a locked switch that looked live and did nothing (G6) | App: PASS. Mockup: FAIL ([T-300](../tasks/T-300.md)) | See "T-270 cleanliness" below. |

### T-270 cleanliness

| Check | Result | Evidence |
|---|---|---|
| Row gone from the app | PASS | No "remote execution" in visible text, `aria-label` or `title` in any of the 279 sweep cases. The distinct visible lines went from 826 to 824: the only two removed are "Allow remote execution" and its tooltip "Always on: remote execution can't be turned off from here." ([`visible-text.txt`](acceptance/2026-11-19-a6/visible-text.txt)). |
| Tunnels has no switch | PASS | [`harness/switch.ts`](acceptance/2026-11-19-a6/harness/switch.ts) on both machine pages of every pair and in `peer-off`, `link-down`, `no-agent` (11 cases): no `role="switch"` named Tunnels or "remote execution"; the Tunnels module is a plain chip. Section screenshots: [`switch/`](acceptance/2026-11-19-a6/switch/). |
| No locked switch left | PASS | Every `role="switch"` in the sweep (39 cases) has no `aria-readonly`; on the machine pages the only ones are Keyboard and mouse and Clipboard, and each one toggles on click and back on Space in all 11 cases, with no console errors ([`switch.txt`](acceptance/2026-11-19-a6/switch/switch.txt)). |
| App code | PASS | `Switch.svelte` lost `locked`, `hint`, `aria-readonly`, `aria-describedby` and `.sw.locked`; `Machine.svelte` lost the `.remote` block, its CSS and the `Switch` import. `svelte-check --fail-on-warnings`: 523 files, 0 errors, 0 warnings (no unused selector). Observation, no task: the global `.sr-only` utility (`src/styles/global.css`, T-112) has had no user since T-270 removed the hint; it is a generic utility, not a switch style. |
| Mockup | **FAIL** | [T-300](../tasks/T-300.md). The removed row was the only `.sw` element of `mockups/v03/V3-Machine.dc.html`, but its four rules stay (lines 29, 30, 68, 69: the track, the knob, `transition: background .25s` and the knob's `left .25s` spring). No other `V3-*` mockup has `.sw` rules. The render is unaffected (PNG byte-identical), but the source of truth still declares a control and two transitions the screen no longer has. |
| Docs | PASS | DECISIONS T-115 line marked superseded, T-270 line added; `mockups/README.md` records the removal with no mask; `machine-b` has no mask in `shots-spec.ts`. The only other mention is the historical `docs/tasks/T-115.md` (done). |

## How it was measured

- `pnpm shots --check` on `dist-demo`: 72 screenshots and the style parity of 22 components, thresholds ≥ 0.99 and worst tile ≥ 0.97. Exit 0, working tree clean afterwards (app screenshots byte-identical to the committed ones). Every similarity is the same as in round 5. Output: [`pnpm-shots.log`](acceptance/2026-11-19-a6/pnpm-shots.log).
- Masks: `apps/tandem-ui/scripts/shots-spec.ts` has not changed since round 5; its 30 mask definitions (each with its reason) are each one of the differences listed in `mockups/README.md` ("What the mockups say and DECISIONS corrects" and the T-120 section). `machine-b` is compared in full, with no mask.
- `pnpm mockups:shot` re-rendered the 72 mockup PNGs: byte-identical to `docs/mockups-png` (including the post-T-270 `V3-Machine.png`).
- Sweep ([`harness/sweep.ts`](acceptance/2026-11-19-a6/harness/sweep.ts), round 5 harness with two T-270 checks added: "remote execution" anywhere in the text, and any switch that is read-only or carries the old "Always on…" title): 279 demo cases, the same as round 5, in normal and reduced motion. Summary: [`sweep-summary.json`](acceptance/2026-11-19-a6/sweep-summary.json); distinct visible lines: [`visible-text.txt`](acceptance/2026-11-19-a6/visible-text.txt) (824).
- Animations ([`harness/motion.ts`](acceptance/2026-11-19-a6/harness/motion.ts), [`motion.txt`](acceptance/2026-11-19-a6/motion.txt)), and the Desk cursor sampled for 8 s with [`harness/probe.ts`](acceptance/2026-11-19-a6/harness/probe.ts) ([`desk-cursor-probe.txt`](acceptance/2026-11-19-a6/desk-cursor-probe.txt)).
- `pnpm test` (51 files, 732 tests; round 5 had 733, T-270 dropped the locked-switch test) and `pnpm check` (523 files, 0 errors, 0 warnings) pass. `scripts/ci/english.sh`, `tasks.sh`, `status.sh` and `privacy.sh` pass on this branch.

## Global rules

| Item | Result | Detail |
|---|---|---|
| G1 Only two machines | PASS | No "Add machine" or third machine in 279 cases. |
| G2 Nothing fixed to Mac/PC | PASS | The only Mac-word hits on `win-win` are `⌘K` in the nine cases the harness opens with a macOS user agent (as in rounds 4 and 5); with a Windows user agent every route shows `Ctrl+K`. No PC words on `mac-mac`, no "System Settings" on a Windows machine. |
| G3 Sidebar | PASS | On all main routes in the three pairs; sidebar regions 100 % on the compared shots, ≥ 99.56 % on the D2 desks. |
| G4 Buttons, icons, chips | PASS | Style parity of 22 components passes (switch on/off and knobs against `F-Settings`). Buttons without an SVG are the same cards, options, module chip switches, radios and steps as in round 5, all drawn that way in the mockups. No colored top line on any card (0 in 279 cases). |
| G5 No filler | PASS | No "Keyboard here", "requires cable", old modes or `tandem.toml`/`[network]` asks. The one `tandem.toml` hit is Settings → Advanced showing the file's location with "Show file", as `F-Settings--advanced` draws it. |
| G6 Everything does something | PASS | The locked switch is gone; every remaining switch toggles (above). |
| G7 Animations | PASS | `motion.txt` matches round 5 except three counts: `machine-b` mockup 13 → 11 and app 18 → 16 (the removed switch's two transitions on both sides), and two app-only counts that vary with the capture moment (`setup-permissions-all` 27 → 29, `palette-no-results` 23 → 25, missing 0 in both). The same 15 cases with gaps as rounds 4 and 5, all checked by hand then (Desk `still` pipes and JavaScript cursor, crossing-test mockup-only buttons and merged waiting step, the duplicated `.go` class in `F-Setup5`, seeded notices without a life bar, `tunnels-empty` not drawn). Live: the Desk cursor runs its loop (x 350 → 669 → 350 in 7 s) and the edge flashes on each crossing (opacity 0.80 at 1.5 s, 0.89 at 5.75 s). The orphan `.sw` transitions of `V3-Machine` are not matched by any element on either side: see P3 and T-300. |
| G8 Reduced motion | PASS | 0 running animations, 0 declared animations or transitions in all 279 cases. |
| G9 Console and text | PASS | 0 console errors or warnings, 0 external requests, 0 harness errors (the round 5 click timeout on `crossing-crossed` is gone: the harness no longer clicks a button that the screen does not have). No `{{`, `undefined`, `NaN` or Spanish. |
| G10 Shared configuration | PART B | |
| G11 Baseline quality | PART C | |

## Screens

| # | Screen | Result | Note |
|---|---|---|---|
| P1 | Desk | PASS | 99.99 % / 98.4 %, with the two scenes too. |
| P2 | Tunnels | PASS | 7 mockup states at 100 % / 100 %; empty and 12 processes informative. |
| P3 | Machine | **FAIL** | App: parity passes (`V3-Machine` 99.99 % / 98.6 % with no mask, `F-MachineMac` 99.99 % / 98.4 %) and the row is gone on every page and pair. Mockup: `V3-Machine` keeps the removed switch's styles ([T-300](../tasks/T-300.md)). |
| P4 | Monitors and scenes | PASS | |
| P5 | Arrangement | PASS | 4 sides × 3 alignments, chosen screen, unsupported: 100 % / 99.7 %. |
| P6 | Crossing test | PASS | 100 % / ≥ 99.7 %. |
| P7 | Network | PASS | 99.98 % / 97.9 %; notices injected with the T-243 texts in the three pairs. |
| P8 | Setup | PASS | Steps 1–6 and variants; admin note per system. |
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
| Machine pages after T-270 | [`mac-win` b](acceptance/2026-11-19-a6/new-screens/r6-machine-b-mac-win.png), [from Windows](acceptance/2026-11-19-a6/new-screens/r6-machine-a-from-windows.png), [`win-win` a](acceptance/2026-11-19-a6/new-screens/r6-machine-a-win-win.png), [`win-win` b](acceptance/2026-11-19-a6/new-screens/r6-machine-b-win-win.png), [`mac-mac` b](acceptance/2026-11-19-a6/new-screens/r6-machine-b-mac-mac.png), [peer off](acceptance/2026-11-19-a6/new-screens/r6-machine-b-peer-off.png), [link down](acceptance/2026-11-19-a6/new-screens/r6-machine-b-link-down.png) |
| Environments section, per pair and state (where the row was) | [`switch/`](acceptance/2026-11-19-a6/switch/) |
| Admin note per system | [`macOS`](acceptance/2026-11-19-a6/new-screens/r6-admin-off-macos.png), [`mac-mac`](acceptance/2026-11-19-a6/new-screens/r6-admin-off-mac-mac.png), [`Windows`](acceptance/2026-11-19-a6/new-screens/r6-admin-off-windows.png), [`win-win`](acceptance/2026-11-19-a6/new-screens/r6-admin-off-win-win.png) |
| States view per system | [`macOS`](acceptance/2026-11-19-a6/new-screens/r6-states-macos.png), [`Windows`](acceptance/2026-11-19-a6/new-screens/r6-states-windows.png), [`win-win`](acceptance/2026-11-19-a6/new-screens/r6-states-win-win.png), [`mac-mac`](acceptance/2026-11-19-a6/new-screens/r6-states-mac-mac.png) |
| Seeded notices, Network notices | [`notices win-win`](acceptance/2026-11-19-a6/new-screens/r6-notices-win-win.png), [`notices mac-mac`](acceptance/2026-11-19-a6/new-screens/r6-notices-mac-mac.png), [`network`](acceptance/2026-11-19-a6/new-screens/r6-network-notices.png), [`network win-win`](acceptance/2026-11-19-a6/new-screens/r6-network-notices-win-win.png), [`network mac-mac`](acceptance/2026-11-19-a6/new-screens/r6-network-notices-mac-mac.png) |

## Screenshots (all)

"INFORMATIVE" marks a state the mockup does not draw (accepted by the ACCEPTANCE rules, no pixel comparison). App images: `docs/screenshots/v03/<name>.png`; mockups: `docs/mockups-png/<mockup>.png`.

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
| machine-b | V3-Machine | 99.99 % | 98.6 % | PASS (pixels; P3 fails on the mockup source, T-300) |
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
| [T-300](../tasks/T-300.md) | `mockups/v03/V3-Machine.dc.html` keeps the four `.sw` rules (track, knob and their two transitions) of the "Allow remote execution" row T-270 removed; no element uses them. |

## Reproducing

```sh
cd apps/tandem-ui && pnpm install && pnpm shots --check          # builds dist-demo
cd ../../tools/mockup-render && pnpm install && pnpm mockups:shot  # mockup references
cd ../.. && node docs/reports/acceptance/2026-11-19-a6/harness/sweep.ts <out>
node docs/reports/acceptance/2026-11-19-a6/harness/motion.ts <out>
node docs/reports/acceptance/2026-11-19-a6/harness/switch.ts <out>
node docs/reports/acceptance/2026-11-19-a6/harness/probe.ts 'demo' '#/desk' '' 8000
grep -n '\.sw' mockups/v03/V3-Machine.dc.html                     # T-300
```
