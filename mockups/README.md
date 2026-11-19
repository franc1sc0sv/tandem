# Mockups

- `main.html`, `next.html`: v0.1 and v0.2 UI (history).
- **`v03/`: interactive visual reference for v0.3** (source of truth for the UI). A copy of the files of the published design (`https://claude.ai/artifact/9iR76c5fczvnpXCbp9BnJt`, private). Each `*.dc.html` is a 1280×860 screen; `canvas.json` gives the title and page of each one.

## How to view them

The `.dc.html` files use a template format (`<x-dc>`, `{{…}}`, `<sc-for>`, `<sc-if>`, class `Component extends DCLogic`) whose original runtime (`support.js`) is not in the repo. `tools/mockup-render` (T-125) ships its own:

```sh
cd tools/mockup-render
pnpm install && pnpm exec playwright install chromium
pnpm mockups:serve     # http://127.0.0.1:4173/: index; the links go from one screen to another
pnpm mockups:shot      # screenshots → docs/mockups-png/
```

- The generated screenshots are in **`docs/mockups-png/`**: `<mockup>.png` is the initial state and `<mockup>--<state>.png` each named interactive state (for example `V3-Arrangement--side-top-center.png`); the list of states and their clicks is in `tools/mockup-render/src/states.ts`.
- Compare an app screenshot with its mockup: `pnpm mockups:compare app.png ../../docs/mockups-png/V3-Desk.png --diff diff.png` (from code: `compareWithMockup(screenshot, { mockup, state, diffPath })` from `@tandem/mockup-render/harness`).
- If you change a mockup, run `pnpm mockups:shot` again and commit `docs/mockups-png/`. The `ui (mockups)` CI job generates them as an artifact and warns if they do not match the committed ones.

More detail in `tools/mockup-render/README.md`.

## Which screen is which

| Mockup | Screen |
|---|---|
| `V3-Desk` | Desk (home) |
| `V3-Compute` | Tunnels |
| `V3-Machine`, `F-MachineMac` | Machine |
| `V3-Monitors`, `F-NewScene` | Monitors and scenes |
| `V3-Arrangement`, `F-CrossingTest` | Arrangement and crossing test |
| `V3-Network` | Network |
| `F-Setup1`, `V3-Setup`, `F-Setup3`…`F-Setup6` | Setup (steps 1–6) |
| `F-Transactions`, `F-Notices` | Scene/power progress, notices |
| `F-Settings`, `F-Palette` | Settings, ⌘K palette |
| `V3-States` | Trouble states |
| `V3-Tray` | Menu bar / tray |
| `F-ClaudeMCP` | What Claude sees (MCP tools) |

## What the mockups say and DECISIONS corrects

- **"Allow remote execution" block of `V3-Machine`** (T-270, user decision 2026-11-19): removed from the mockup itself (`V3-Machine.dc.html` and its PNG), and from the app's Machine page. Tunnels has no switch (DECISIONS T-098) and the Modules list already shows it as a chip, so nothing is masked and `machine-b` is compared in full.
- **Third machine** (`F-AddHost`, the "MacBook Air", "Add machine" in the sidebar, "Desk of N machines" in the notes): out because of D1. In its place, Settings → "Change the other machine" (pair again).
- **"Add machine" card of `V3-Desk`** (D1): masked in the comparison (200 × 210 px to the right of the machine cards); in the app that gap stays empty, there is no card to mask. Power on / Power off live inside the other machine's card (which is compared), styled like the mockup's buttons.
- **Palette shortcut** (`⌘K`): the mockups are drawn on macOS; the app shows `Ctrl+K` on Windows. The screenshot harness uses a macOS user agent.
- **"Install" button of the admin service** (`F-Setup1`) (T-131): no longer masked; the app draws it like the mockup. It works on macOS (LaunchDaemon, admin dialog) and on Windows (elevated task with UAC and an ACL check); if it does not apply (per-user install, no `.app`), the row explains why and does not offer the button (`?admin=off`). The demo leaves the service pending (`?admin=pending`) so the row shows the same spinner as the mockup.
- **"macOS 26 · Apple M5 · 24 GB"** and other sample data: in the app they come from the real scan (D10).
- **"Simulate…" buttons** (dotted border): mockup only; in the app those steps really happen. Masked regions (`F-Setup5`, "waiting"): the "Simulate crossing" and "Simulate other edge" boxes.
- **`F-MachineMac` without sidebar** (G3/D16): it is a flow screen with the "Desk › MacBook Pro" path on top; the app has the sidebar. Only the content is compared: from the corner of its `header` to the bottom edge (in the mockup, `x 32, y 55`, 1216 × 805 px; the app is captured at 1492 px wide so its content measures the same). The path and the sidebar stay out of the comparison.
- **Color picker on this machine's page** (D17): `F-MachineMac` does not draw it; the app has it on both pages. Masked region: the "Machine color" `radiogroup` in the header (four 22 px circles, 106 × 22 px).
- **Module state in `F-MachineMac`** (G4): the mockup writes it as loose text ("active", "off", "partial"); the app uses the uniform chip (a switch if it can be turned off; its text is "off"). Masked region: the state of each module row (`.mod > .chip` in the app, `.mod > span:last-child` in the mockup); the "Open settings" button of the "Permissions" row is compared.
- **"Latency · 10 min"** (`V3-Network`, latency chart): the agent keeps 60 s per path (DESIGN §6, T-102); the app says "Latency · 60 s" and the harness masks that label in both screenshots.
- **`F-Transactions`: "Stop waiting"**: the agent has no command to cancel a power-on, so the app does not show the button (same rule as "forget monitor"). Masked region: the last block of the power card in the mockup.
- **`F-Transactions`: "from the MacBook"**: the mockup names the machine by hand; the app says the real name (D10).
- **`F-Setup5`: the repeated `.cur` class**: the mockup CSS declares `.cur` for the drawing's cursor (14 × 14, absolute) and the current step of the list carries the same class (`.stl.cur`): it leaves the flow, its text splits into three lines and "Done" moves up 44 px. It is a mockup defect; the app draws the list like the other screens. Masked regions in the four `F-Setup5` screenshots: 140 × 60 px at (10, 138) and 300 × 94 px at (0, 258), the affected part of the step list.
- **`F-Setup5`: "from the MacBook to the PC"**: the mockup names the machines by hand; the app says the desk's names (D10). Masked regions: in the intro paragraph, only the names on the second line (552 × 20 px at (438, 134); the rest is compared) and, while waiting for the crossing, the verdict ("Waiting for the crossing over the right edge of…", 440 × 40 px at (350, 482)).
- **`F-Setup3`: "Restart agent"**: the mockup does not offer to restart the agent after opening a macOS panel (T-090: the permission is granted to the process, which must restart). The app offers the restart. Masked region in the "one granted" state: the 200 × 40 px footer at (775, 770).

## Regions the harness masks in T-120 (Settings, States, Palette, Tray, Notices)

Rule: only a box that cannot match (real data, a function that does not exist) is masked, and never a title, text or button the app does have. Each region comes with its reason; the source is `apps/tandem-ui/scripts/shots-spec.ts`.

**States (`V3-States`).** `?demo&states` seeds the mockup's sample texts (names, figures, the `epoch 9/20 · loss 0.0371` line), so titles, texts and buttons of the nine cards are compared without masking; card heights come from the app's real CSS, with no measurement adjustments. Masked region:
- "Send over Wi-Fi anyway" button of the "Link" card: the agent has no such function, and without a function there is no button.
- "First time" card (D1, T-142): only its sentence and its button label are masked; the app says "Pair the other machine" and "…shows as off until you pair the other machine" instead of "Add machine" / "Add another machine to use this" (there is no third machine). The title, label and icon are compared.

**Setup, "Close" button (T-156).** The `F-Setup*` screens draw no close button; the app adds a single one with an "x" icon (`aria-label="Close"`, 32 px, visible focus) at the foot of the side panel, because without it someone using only the mouse cannot leave in the middle of setup (usability wins). It is a deliberate, minimal difference: the similarity of the `F-Setup*` screens stays at ≥ 99 % with no new masks.

**Setup, "Create a new desk" (`V3-Setup--new`, T-145).** The mockup shows the "Desk name" field; the app does not, because the name does not exist in the agent nor travel in the protocol and a field that saves nothing violates DESIGN §3.8. Instead the app shows the pairing code (pairing is symmetric, T-097) and the note "Then, on the other machine…". Only the panel below the two options (`setup-desk-new`) is masked; the options, title and buttons are compared.

**Settings (`F-Settings`).**
- Desk name: the mockup has an editable field and the app shows the text, because the name does not travel with the desk and a field that saves nothing violates DESIGN §3.8. Only that box is masked.
- "Leave desk" button (mockup): D1, exactly two machines. For the same reason, the other machine's pencil falls on the right edge of the row in the app; only that pencil is masked.
- "Add a machine · Show code" row (D1): the app says "Change the other machine · Pair again". The label and the button are masked, not the row.
- Shortcuts: no masked regions. The app has all six shortcuts and `settings-shortcuts` measures 100 %.
- Advanced: the MCP server's session count; the agent does not count them yet.

**Palette (`F-Palette`).** `?demo&palette` seeds the mockup's data (the "Work" scene on the Mac, the third scene "PC only" and one tunnel per project, "ia-local" and "tandem"), and the tunnel path uses the same labels as the list ("Mac → PC"), so scenes, tunnels, color dots and the "Monitors" and "Network" lines are compared without masking. The only masked region: "Add machine…" (D1; the app says "Change the other machine…"). States `filter` ("only") and `no-results` ("zzz"): the text is typed in the field and compared as usual. Besides titles, the app searches the group name ("sce" leaves all the scenes; the mockup only looks at the title) and adds the machine pages under "Go to"; that is why the `filter` state uses a text that gives the same result in both.

**Tray (`V3-Tray`).** `?demo&tray` seeds the mockup's data: a 0.4 ms link (the Desk says 0.3), Mac load at 18 %, "PC only" as the third scene and tunnels in three projects (two active, "notes" idle). The "Mac → PC" labels come from `shortLabels`, as in Tunnels. The only masked region: "Lock here" (without the action in the IPC there is no button; T-112, G6).

**Notices (`F-Notices`).** The full stack is compared (the three notices with their tones: warning, success and Claude), with no masked regions: `?demo&notices` seeds the mockup's texts, including the "Send over Wi-Fi anyway" button, which in the demo does nothing (the real app does not show it) so the stack measures the same. Measuring technique: the app draws the stack as a fixed layer in the window corner and the mockup stacks it in the flow inside a frame; so the text lands on the same pixel, during capture the real page is hidden (`.shell`; in the mockup it is a skeleton), the frame's background is painted and the stack goes to absolute position. It is cropped from the corner of the first notice in each image. The "Changed from… · Undo" notice (D5) is not in `F-Notices`: the app's tests cover it.

**Mockup regions that are not compared.**
- `F-Notices` is measured by cropping the notice stack (390 × 681 px from the first notice). Out of it: the "Notices" header with the "Add notice" button (mockup demo controls) and the right column "System notifications" ("Only for what happens while you are not looking at Tandem…", with three notifications, one of them "MacBook Air joined "My desk"", third machine, D1). It is not implemented: DECISIONS D14 leaves audio and notifications out for now, and the app has no system-notification code. If the user decides on them, it will be a new decision in DECISIONS and then it will be measured.
- `F-ClaudeMCP` is Claude's view, not an app screen: it is tested with the `tandem mcp` tools (part B of the acceptance).
- The other screenshots (`shots-spec.ts`) were reviewed: the rest of each mockup is covered by some screenshot or region, with its reason in the sections above.
