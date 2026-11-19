# Tandem v0.3 acceptance, part A, round 4 (2026-11-19)

Part A of 3: visual parity, animations and visible text of every screen. Base: `origin/develop` at `8504769` (T-240). `develop` later moved to `92a1dc1` (T-242, T-260, T-261 and the part C report), which only touches docs, CI scripts and `scripts/hw`; nothing under `apps/tandem-ui`, `mockups` or `tools`, so the results below hold for it. Out of scope: part B (flows with real agents) and part C (baseline quality, CI, G10, G11).

**Result: `ACCEPTANCE: FAIL 3`** (tasks T-243, T-244, T-245). Every screen and state is at parity and motion and reduced motion pass. The failures are in what develop added since round 3 and in demo data: the new Network notices (T-223) tell the user to edit `tandem.toml`, the demo still ties some texts and controls to Mac/PC in the `win-win` and `mac-mac` pairs, and the agent's new "Protected screens" line (T-027) is broken English.

## Round 3 failures, re-checked

| Task | Finding | Result | Evidence |
|---|---|---|---|
| T-211, T-212, T-213 | Spanish left in the repository | PASS (part A) | Merged in `b14085f` (T-210, CI guard). The 261-case sweep found no Spanish in the app (826 distinct visible lines, [`visible-text.txt`](acceptance/2026-11-19-a4/visible-text.txt)). The repository-wide check belongs to part C. |

## How it was measured

- `pnpm shots --check` on `dist-demo`: 72 screenshots and the style parity of 22 components. Thresholds: similarity ≥ 0.99 and worst tile ≥ 0.97. Exit 0. Full output: [`pnpm-shots.log`](acceptance/2026-11-19-a4/pnpm-shots.log). The run left the working tree clean, so the app screenshots came out byte-identical to the committed ones (`docs/screenshots/v03`, `similarity.json`).
- Mockup references: `pnpm mockups:shot` re-rendered the 72 mockup PNGs after T-221 changed four mockups. All came out byte-identical to `docs/mockups-png`, so the comparison is against current mockups.
- Text and rule sweep ([`harness/sweep.ts`](acceptance/2026-11-19-a4/harness/sweep.ts), summary in [`sweep-summary.json`](acceptance/2026-11-19-a4/sweep-summary.json)): 261 demo cases. They cover every `shots-spec` case, the 12 routes × `here=a/b` × three pairs (`mac-win`, `win-win`, `mac-mac`), four trouble states × five routes × three pairs, the six setup steps, pairing code and rejection, the palette, the tray, the notices and "Changed from…" per pair, plus 18 cases for the screens that are new since round 3. For each case the sweep dumps the visible text, `aria-label`, `title`, `placeholder` and `alt`, and checks them for Spanish, `{{`, `undefined`, `NaN`, `null`, `[object`, forbidden phrases (Keyboard here, Add machine, requires cable, third machine, modes, `PC → PC`), Mac words on `win-win` and PC words on `mac-mac`, X/Y in the arrangement, console errors and warnings, and external requests. It also lists buttons without an icon, chips by height and radius, and cards with a colored top line (a thicker or differently colored top border, or a thin top pseudo element).
- Reduced motion: each of the 261 cases again with `prefers-reduced-motion: reduce`: 0 running animations, 0 elements with an animation or a transition of non-zero duration.
- Animations ([`harness/motion.ts`](acceptance/2026-11-19-a4/harness/motion.ts), [`motion.txt`](acceptance/2026-11-19-a4/motion.txt)): for every `pnpm shots` case, the declared animations (duration, iterations, animated properties from the `@keyframes`) and transitions (property, duration) of the mockup state after its clicks, against the app after the same clicks. Every reported gap was then checked by hand with [`harness/probe.ts`](acceptance/2026-11-19-a4/harness/probe.ts) (see G7).
- New screens: injected states (`inject=` in the sweep) rewrite the demo bundle in the route handler. The Network notices use the exact texts `link_notices` builds (T-223). Setup step 4 gets a hook that plays a key heard on the other machine, which is what `key_probe { second }` does live (T-220).
- `pnpm test` (48 files, 682 tests) and `pnpm check` (519 files, 0 errors, 0 warnings) pass.

## Global rules

| Item | Result | Detail |
|---|---|---|
| G1 Only two machines | PASS | No "Add machine" or third machine in the 261 cases. |
| G2 Nothing fixed to Mac/PC | FAIL | [T-244](../tasks/T-244.md). Demo data, same class as T-170. The seeded notices name "PC-Desktop", "MacBook Pro" and "WSL" in `win-win` and `mac-mac`. `admin=off` shows the Windows "Program Files" note on the macOS machine. The States reference view shows a permissions card with "Open System Settings" on Windows machines, and the real `open_settings` there returns "these permissions only exist on macOS". The routes, setup, palette, tray and trouble states are clean in all three pairs. The `⌘K` hits on `win-win` trouble and setup cases came from the harness's macOS user agent, not from the app; with a Windows user agent every route shows `Ctrl+K`. |
| G3 Sidebar | PASS | Present on all main routes in the three pairs; sidebar regions ≥ 99.56 % (`desk-win-win`, D2 colors) and ≥ 99.98 % on the compared shots. |
| G4 Buttons, icons, chips | PASS (with T-243) | Style parity of 22 components passes. Every button without an SVG is a card, option, switch, radio or step drawn the same way in the mockup (scene cards, palette rows, color dots, step list, screen tiles). No colored top line on any card in 261 cases. The new Network notice has no icon, unlike every other warning: part of T-243. |
| G5 No filler | PASS | No "Keyboard here", "requires cable" or old modes. |
| G6 Everything does something | FAIL (demo) | [T-244](../tasks/T-244.md). In the demo, Monitors, Tunnels / Remote compute and Protected screens get a switch the agent never offers (T-098). The Windows permissions card is in the same task. |
| G7 Animations | PASS | See below. |
| G8 Reduced motion | PASS | 0 / 0 / 0 in all 261 cases. |
| G9 Console and text | FAIL | [T-245](../tasks/T-245.md): the agent's "Protected screens" line is broken English. Zero console errors, warnings and external requests in the 261 cases. One harness click timeout (`new:crossing-crossed`: the crossing screen starts its test without a "Test crossing" button); that state is covered by the `crossing-crossed` shot. |
| G10 Shared configuration | PART B | D4 on the screen: the T-243 notice asks the user to edit `tandem.toml`. |
| G11 Baseline quality | PART C | |

### G7 detail

The motion comparison reports 0 gaps for 57 of the 72 cases. The rest, checked by hand:

- **Desk (`V3-Desk`, five cases): cursor and edge `7 s`, pipes `1 s / 1.3 s`.** The shots use `?demo&still`, which slows the pipes (1.8 / 2.34 s) and stops the JavaScript cursor. Without `still`, the pipes are `fl 1s` and `fl 1.3s`, as in the mockup. The cursor (T-140) was sampled every 250 ms for 8 s: it runs a 7 s loop from x=350 to x=671, crosses and comes back, and the edge flashes on each crossing (opacity up to 0.89 at t=1.5 s and 0.86 at t=6.0 s). PASS.
- **Crossing (`F-CrossingTest`, four cases).** The transitions at 0.18 s and the SVG transform at 0.3 s belong to the mockup-only "Simulate…" buttons (masked). The `nud 1.2s`, `eg 1.4s` and `cnt 20s` "gaps" in the crossed and no-cross states come from the harness merging the mockup's waiting step into those states: the app's waiting state has `cnt 20s`, `eg 1.4s ∞` and `nud 1.2s ∞`, plus `in 0.35s`. PASS.
- **Setup step 5 (`F-Setup5`, three cases): `mv 2.2s ∞ left,opacity`.** In the mockup the class `.go` is used twice: on the drawing cursor and on the arrow of the "Continue" button. The cursor exists in the app as `wz-mvx 2.2s ∞` (transform). The blinking "Continue" arrow is a side effect of that duplicated class, the same kind of artifact as the `.cur` one the harness already masks. PASS.
- **Notices: `life 6s` vs 1.1 s.** The seeded demo notices are held for the capture and have no life bar. Real notices carry `.life` with `animation: life 6s linear` (`Toasts.svelte`). Known since round 1. PASS.
- **`tunnels-empty`:** not drawn by the mockup (informative).

## Screens

| # | Screen | Result | Note |
|---|---|---|---|
| P1 | Desk | PASS | 99.99 % / 98.4 %, also with the two scenes. |
| P2 | Tunnels | PASS | The 7 mockup states 100 % / 100 %; empty and 12 processes are not drawn (same components). |
| P3 | Machine | FAIL | Parity passes: `V3-Machine` 99.99 % / 98.6 %, `F-MachineMac` 99.99 % / 98.4 %. Failures: the live Protected screens line ([T-245](../tasks/T-245.md)), and the demo switches on modules that cannot be switched ([T-244](../tasks/T-244.md)). |
| P4 | Monitors and scenes | PASS | |
| P5 | Arrangement | PASS | 4 sides × 3 alignments, chosen screen, unsupported. |
| P6 | Crossing test | PASS | "Crossed correctly" (T-221) is in both the mockup and the app, 100 % / 99.7 %. |
| P7 | Network | FAIL | Parity 99.98 % / 97.9 % ("Primary", "Bandwidth" from T-221 in both). The T-223 notices fail: [T-243](../tasks/T-243.md). |
| P8 | Setup | PASS | Steps 1 to 6 and variants. New states checked below. |
| P9 | Progress / power | PASS | "Save the current state" and "Rolling back to the previous state" (T-221) match the mockup, 99.88 % / 97.9 %. |
| P10 | Notices | PASS (`mac-win`) | 99.97 % / 99.4 %. The seeded texts in the other pairs: [T-244](../tasks/T-244.md). |
| P11 | Settings | PASS | Five tabs; "Update automatically" (T-221) in both. |
| P12 | Palette | PASS | |
| P13 | Trouble states | FAIL (demo, `win-win`) | `mac-win` 100 % / 99.9 %. The Windows permissions card: [T-244](../tasks/T-244.md). |
| P14 | Tray | PASS | 99.93 % / 98.0 %. |
| P15 | What Claude sees | PART B | |

## New since round 3

Screenshots in [`new-screens/`](acceptance/2026-11-19-a4/new-screens/). None of these states is drawn by a mockup, so they are judged by components, tokens and meaning, not by pixels.

| Change | Screen and state | Result | Detail |
|---|---|---|---|
| T-027 | Setup step 1, administrator service row: pending (`setup1-admin-pending`), after "Install" (`setup1-admin-installed`), already installed (`setup1-admin-ok`), Windows pair (`setup1-win-win-admin-pending`) | PASS | The pending row shows the primary "Install" button with its icon, the same as `F-Setup1` (the `setup-machine` shot at 99.99 % / 98.2 % uses this state). After "Install" the row turns ok and the button goes away. On Windows the row and the button are the same; T-027 only changed what the elevated install does (`--install --with-service`). |
| T-027 | Setup step 1, `admin=off` (`setup1-admin-off`) | FAIL | The Windows note on a macOS machine: [T-244](../tasks/T-244.md). |
| T-027 | Machine page, "Protected screens" (`machine-windows-protected-screens`, `machine-win-win-protected-screens`) | FAIL | The demo row matches `V3-Machine`. The real agent's line is "The mouse and keyboard also on UAC prompts and at sign-in" ([T-245](../tasks/T-245.md)). The demo gives it a switch the agent does not ([T-244](../tasks/T-244.md)). It is absent on macOS machines, which is correct. |
| T-223, T-237 | Network with the two notices `link_notices` builds (`network-notices`, `network-notices-win-win`) | FAIL | [T-243](../tasks/T-243.md): "Remove the extra peer from [network] in tandem.toml" (D4, and wrong when the extra address was discovered), a handled condition shown as trouble, no icon, and a message in lower case. The rest of the screen is unchanged; with the notices it stays in English, with no console output. T-237 adds no UI of its own: a one-way link shows as a normal link. |
| T-220 | Setup step 4 activity: none (`setup4-idle`), this machine (`setup4-activity-here`), the other machine (`setup4-activity-other`), both (`setup4-activity-both`) | PASS | Each card adds its own "Activity detected" row (same `wz-dev` row and check icon) when its machine is heard, independently. Observation, no task: in the demo the other machine has no devices, so its card says "No keyboard or mouse right now" and "Activity detected" together; live, a heard key comes with the device that sent it. |
| T-221 | Crossing, Network, Progress, Settings → Updates, Setup step 5 | PASS | Every changed string is the same in the mockup and the app, and the shots of those screens pass. |

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
| [T-243](../tasks/T-243.md) | Network notices (T-223) tell the user to edit `tandem.toml` (D4). The advice is wrong for discovered addresses, a handled condition shows as trouble, there is no icon and the message starts in lower case. |
| [T-244](../tasks/T-244.md) | Demo tied to Mac/PC in `win-win` / `mac-mac`: seeded notices, the `admin=off` note, the Windows permissions card with "Open System Settings", and switches on modules the agent cannot switch. |
| [T-245](../tasks/T-245.md) | The agent's "Protected screens" line (T-027) is broken English and differs from `V3-Machine`. |

## Reproducing

```sh
cd apps/tandem-ui && pnpm install && pnpm shots --check          # builds dist-demo
cd ../../tools/mockup-render && pnpm install && pnpm mockups:shot  # mockup references
cd ../.. && node docs/reports/acceptance/2026-11-19-a4/harness/sweep.ts <out>
node docs/reports/acceptance/2026-11-19-a4/harness/motion.ts <out>
node docs/reports/acceptance/2026-11-19-a4/harness/probe.ts 'demo' '#/desk' '' 8000
```
