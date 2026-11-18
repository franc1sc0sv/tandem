# Tandem v0.3 acceptance (definition of "done")

The app is **done** only when the `acceptance` agent (`.claude/agents/acceptance.md`) answers `ACCEPTANCE: FINAL_OK` with **all** the items in this file at PASS, in a single run over `origin/develop`. Nobody else (neither the captain nor an implementer) may declare the app done.

Notation: `[hw]` = needs the real hardware (covered by the T-123 suite or left in the pending hardware list of its task with the reason). Each screen is compared with its mockup in `mockups/v03/` (threshold: global similarity **≥ 0.99** and worst 64×64 px tile, `worstTile`, **≥ 0.97**; global similarity alone lets a wrong state pass).

## 0. Global rules (checked on every screen)

- [ ] G1. Only two machines; no text, button or flow for a third (D1).
- [ ] G2. Nothing tied to Mac/PC: names, icons and colors come from each machine; tested with a simulated Windows+Windows and macOS+macOS pair (D2, A1).
- [ ] G3. Sidebar on all main screens: Desk, Tunnels (counter), Monitors, Arrangement, Network, Machines (one per machine, with its color), Settings (D16).
- [ ] G4. Single button system (fixed height, always an icon, elevation on hover, glow on the primary, visible focus); icons on every action; uniform status chips; cards without a colored line on top.
- [ ] G5. No filler text or superfluous explanations; no "Keyboard here"; no "requires cable"; no old modes.
- [ ] G6. Every interactive element does something real and verifiable in the agent; nothing decorative that looks like a button.
- [ ] G7. **Animations:** every `@keyframes` and every `transition` of the screen's mockup has its equivalent in the app (same effect, approximate duration and curve); card entrances, screen changes and state changes animated.
- [ ] G8. With `prefers-reduced-motion: reduce` there are no animations.
- [ ] G9. No console with errors; no visible `{{`, `undefined` or `NaN`; all visible text is in English (no Spanish anywhere in the app).
- [ ] G10. Configuration: everything editable is edited from the UI and reaches the other machine; the "Changed from X · Undo" notice works (D4, D5).
- [ ] G11. Quality: fmt, clippy, Rust tests, `pnpm test/check/build` and `develop` CI green on Linux, Windows and macOS.

## 1. Screens (visual parity ≥ 0.99 + everything interactive works)

| # | Screen | Mockup(s) and states to compare |
|---|---|---|
| P1 | Desk | `V3-Desk` |
| P2 | Tunnels | `V3-Compute`: no tunnels, project collapsed/expanded, tunnel with 1 and with 12 processes, closed tunnel |
| P3 | Machine | `V3-Machine`, `F-MachineMac` |
| P4 | Monitors and scenes | `V3-Monitors` (each input selected), `F-NewScene` |
| P5 | Arrangement | `V3-Arrangement`: 4 sides × 3 alignments, each screen selected, unsupported screen |
| P6 | Crossing test | `F-CrossingTest`: waiting, crossed fine, "Did not cross in 20 s" (timeout: `F-CrossingTest--no-cross`) |
| P7 | Network | `V3-Network` |
| P8 | Setup | `F-Setup1`, `V3-Setup`, `F-Setup3`, `F-Setup4`, `F-Setup5`, `F-Setup6` |
| P9 | Scene / power progress | `F-Transactions`: in progress, success, rollback |
| P10 | Notices | `F-Notices` |
| P11 | Settings | `F-Settings`: each tab |
| P12 | ⌘K palette | `F-Palette` |
| P13 | Trouble states | `V3-States`: no agent, other machine off, link down, permissions revoked |
| P14 | Menu bar / tray | `V3-Tray` |
| P15 | What Claude sees | `F-ClaudeMCP` (the listed tools exist in `tandem mcp`) |

States the mockup does not draw (Tunnels empty and with 12 processes, crossing test through another edge, "Crossed through another edge": the mockup does not draw it): accepted if they use the same components, tokens and animations of the screen, without pixel comparison.

Differences allowed with the mockup: those in `mockups/README.md` ("What the mockups say and DECISIONS corrects") and real data instead of the sample data. Nothing else.

## 2. End-to-end flows (two real agents over loopback + UI)

- [ ] F1. **First use:** setup 1→6 complete on both machines: name and color, pair with a 6-digit code (both UIs show the same code), permissions verified (double in CI; real `[hw]`), keyboard and mouse detected on each machine, arrangement + crossing test that passes with a real `CursorSample` from the engine, verified summary and copyable MCP command.
- [ ] F2. **Arrangement:** move the other machine to each side and alignment, save, verify on the other machine and in the input engine's geometry (simulated crossing through the new edge passes; through the old one it does not).
- [ ] F3. **Scenes:** create, edit, apply successfully, apply with an injected failure → visible rollback; global scene shortcut.
- [ ] F4. **Monitors:** fake standard monitor: change input with verdict; factory monitor: "not supported", with no writes at all; owner re-election when the owner powers off. `[hw]` real 34" with automatic revert.
- [ ] F5. **Tunnels through MCP:** an MCP client opens a tunnel with a git workspace, runs 3 processes at once, reads output, copies files, opens a port, cuts the link and recovers it without losing output, `bring_back` creates the branch; the UI shows everything live in read-only. `[hw]` PC↔Mac over Wi-Fi.
- [ ] F6. **Network:** cut the main path → input moves to the next in < 1 s, event visible in Network; "Measure bandwidth" gives a value; data paused if input degrades.
- [ ] F7. **Keyboard and mouse with no owner:** simulated physical input on A and then on B: the one in use is in charge; no stuck keys when switching or when the link drops. `[hw]` real latency over Wi-Fi.
- [ ] F8. **Clipboard and files:** copy text/image on A and paste on B; dragging a file across the edge leaves it on B (doubles in CI; real `[hw]`).
- [ ] F9. **Shared configuration:** change name, color, shortcuts and preferences on A → they reach B; a simultaneous change converges; Undo restores.
- [ ] F10. **Power:** power off the other machine (transaction with progress) and power it on (simulated Wake-on-LAN); lost other machine → correct trouble state.
- [ ] F11. **Modules:** turn a module of the other machine off/on from its page; "unavailable" reasons correct.
- [ ] F12. **Settings and palette:** every Settings action and every palette command does what it says.
- [ ] F13. **Updates:** look up a version against a test `latest.json`, plan to update both machines (double); the real release stays documented.
- [ ] F14. **Tray:** open the app, apply a scene and see the state from the menu bar/tray.

## 3. Hardware (`[hw]`, T-123 suite)

- [ ] H1. Agents installed on the PC and on the Mac pair over Wi-Fi.
- [ ] H2. PC→Mac and Mac→PC tunnel with real processes.
- [ ] H3. Supported 34": state reading and input change with automatic revert.
- [ ] H4. Real cursor crossing with synthetic input in both directions.
- [ ] H5. USB-C: pending the TB4/USB4 cable (A6); does not block FINAL_OK if documented.
