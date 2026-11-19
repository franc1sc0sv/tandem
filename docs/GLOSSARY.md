# Glossary

The repository is entirely in English (see `CLAUDE.md`, "Language"). This file fixes the terms so the UI, the mockups, the code and the docs say the same thing. For anything not listed, pick the plain, short English word a desktop app would use and stay consistent within your change. `scripts/ci/english.sh` fails the build if Spanish comes back.

## Domain terms

| Term | Meaning or usage |
|---|---|
| desk | The pair of machines working as one. "My desk". |
| machine | One of the two computers. Never "host" or "computer" in the UI. |
| peer | The other machine, in code. In the UI: "the other machine". |
| arrangement | The screen layout of both machines. Screen title: "Arrangement". |
| scene | A saved combination of monitor inputs and settings for the desk. |
| tunnel | A forwarded connection to the other machine. "1 tunnel", "2 tunnels". |
| remote compute | Running work on the other machine. |
| environment | WSL, zsh, PowerShell... |
| link | The connection between the machines: USB4, Ethernet, Wi-Fi. |
| network | The network screen and the paths the link can take. |
| pair / pairing | Trusting the other machine. "Pair the other machine". |
| pairing code | The short code compared on both machines when pairing. |
| key / credential | The identity of a machine and what it trusts. |
| leave desk | The action that takes a machine out of the desk. Label: "Leave desk". |
| crossing / cross | The cursor moving to the other machine. |
| crossing test | The check that the cursor crosses the marked edge. Button: "Test crossing". |
| edge | The border of a screen where the cursor crosses. |
| keyboard and mouse | The shared input devices. |
| activity detected | The input indicator shown when a device is used. |
| tray | The menu bar / system tray entry. |
| command palette | The Ctrl/Cmd+K window to run actions. |
| setup | The first-run flow. UI: "Setup"; code keeps `wizard`. |
| settings | The preferences screen. |
| power | Power on / power off / Wake-on-LAN. |
| modules | The parts of a machine that can be turned on or off. |
| monitors | The external displays. |
| input (of a monitor) | The video input a monitor shows. |
| clipboard | The shared clipboard module. |
| files / drag | Dragging files between machines. |
| lock / locked | Keeping the cursor on one machine. |
| notices | Toasts and messages shown in the UI. |
| transaction / rollback | A multi-step change that is undone if a step fails. |
| trouble states | The screens that show what is wrong. |
| permissions | The operating system permissions the app asks for. |
| progress | The screen with changes in progress. |
| updates | The update check and install. |
| shortcuts | Keyboard shortcuts (the lib module is `hotkeys.ts`). |
| agent | The background process, `tandemd`. |
| unavailable | A module or action that cannot be used right now. |
| off | A module that is turned off. |

## Docs and tasks

| Term | Usage |
|---|---|
| Context | Task section: why the task exists. |
| Acceptance criteria | Task section: the checklist. |
| Expected fix | Task section of a finding: what should change. |
| Notes | Task section: what was done or left out. |
| Pending hardware verification | Task section: checks only the real machine can do. |
| Open tasks | The list of tasks not done yet. |
| PASS / FAIL | Result of an acceptance item. |
| screenshots | Images of the app, in `docs/screenshots`. |
| similarity | Share of matching pixels between a screenshot and its mockup. |
| worst tile | The lowest similarity of any tile of the compared image. |
| mask | A covered region left out of a comparison. |
