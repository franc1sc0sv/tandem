# Tandem v0.3 acceptance — part B (end-to-end flows)

Date: 2026-11-18 · Base: `origin/develop` at `04f2967` (T-131) · Agent: acceptance (part B of 3). Out of scope: pixel-by-pixel parity (part A) and baseline CI quality (part C).

## Verdict

**FAIL.** 136 checks run, 120 pass and 16 fail; the failures are grouped into 10 fix tasks (T-150 to T-159). A single failing item is enough not to declare the app finished.

## How it was tested

- Two real `tandemd` over loopback (real QUIC, orchestrator, IPC) with the production UI (`pnpm build`) in Chromium with Playwright. Since WebKitGTK is not in the environment, the Tauri glue was replaced by an **HTTP bridge** (`apps/tandem-ui/src-tauri/examples/acc_bridge.rs`) that uses the app's real code (`session`, `shortcuts`, `setup`, `files`, `admin_service`, `update`) and delivers events over SSE; the window sees a test `__TAURI_INTERNALS__`. Each machine has its own bridge and browser.
- Agents: (1) the real `tandemd --sim-backends` binary (pairing, saved arrangement, real tunnels through `tandem mcp`); (2) since `--sim-backends` has no input engine, monitors or clipboard, a **host** (`crates/tandemd/examples/acc_host.rs`, the same agent with `daemon::start`) with the repo's test doubles: input engine with fake backends, the fake DDC bus from `monitor-ddc` (standard and factory, with an injectable failed switch), fake clipboard and a declared operating system (`--os mac|windows`). A control port acts as the "physical world" (move the mouse, plug in the hub, add or remove monitors).
- Network with two and three paths: UDP proxies between the agents (usb4, ethernet, wifi) that can be cut and healed at will.
- The MCP client is a JSON-RPC script against `tandem mcp`.
- Real effect checked on the agent (state over IPC, `tandem.toml`, `ui.toml`, `update.toml`, the fake monitor's bus, `sim-steps.jsonl`, the workspace directory, git branches), not only on screen.
- Pairs tested: macOS+Windows (declared), Windows+Windows and macOS+macOS (G2). Demo mode: all routes and variants.
- Harness code: `docs/reports/acceptance/2026-11-18-b/harness/`. Screenshots: `docs/reports/acceptance/2026-11-18-b/<flow>/<step>.png`.

### Environment limits (not app failures, but left unverified)

- WebKitGTK, the Tauri window, the tray icon and the system's global shortcuts do not exist here: the bridge records what the app would ask of the window (`ui_show_main`, `ui_quit`, `files_open`). The real updater (Tauri plugin) was replaced by a double with the same plan (`update::plan`/`run`) against a test `latest.json`; nothing is installed.
- Real macOS permissions, UAC, real Wake-on-LAN, real DDC, crossing with a real mouse and file drag with a real compositor: `[hw]`. File drag between machines was checked with its integration tests with doubles (`cargo test -p tandemd --test drag --test files`: 4 tests pass), not through the UI.
- "Data paused if input degrades" (F6): checked with `cargo test -p tandem-sim --test network` (4 tests pass, including `measurement_is_cut_if_input_exceeds_its_target`); the UI shows the "Wi-Fi only" case ("Data transfers wait for the cable").
- The hardware suite (T-123) was not run: H1–H5 are out of this part.

## Verdict per item of `docs/ACCEPTANCE.md`

| Item | Verdict | Tasks | Evidence |
|---|---|---|---|
| F1 First use (setup 1→6, pairing, permissions, keyboard and mouse, arrangement + crossing, summary, MCP) | **FAIL** | T-150, T-151, T-156, T-158 | Pairing with a 6-digit code and rejection of a bad code pass; missing: setup only on first use (Windows), step 4, crossing test, summary and "Close" |
| F2 Arrangement (4 sides × 3 alignments, engine) | **FAIL** | T-150 | All 12 cases are saved and the engine crosses through the new edge and not the old one; the UI does not draw the real screens and the crossing test cannot be completed |
| F3 Scenes (create, edit, apply, injected failure, global shortcut) | **PASS** |  | F3-scenes |
| F4 Monitors (standard with verdict, factory without writes, owner re-election) | **PASS** |  | F4-monitors · the real 34" `[hw]` stays in the T-123 suite |
| F5 Tunnels through MCP (git, 3 processes, output, files, port, link cut, bring_back, T-130) | **FAIL** | T-154, T-155 | Everything on the agent passes; a freshly opened UI does not show the existing tunnels and files_put of a credential gives the wrong error |
| F6 Network (cut < 1 s, event in Network, measure bandwidth) | **PASS** | T-153 (related) | F6-network · the measure button stays disabled after reloading on Network (T-153) |
| F7 Keyboard and mouse without an owner | **PASS** |  | F7-no-owner |
| F8 Clipboard and files | **PASS** |  | F8-clipboard · file drag through tests with doubles |
| F9 Shared configuration and "Undo" | **PASS** |  | F9-shared-config |
| F10 Power | **PASS** | T-159 (related) | F10-power · with `[power.wol]` set by hand; without it "Power on" does not work (T-159) |
| F11 Modules (power off/on, "unavailable" reasons) | **FAIL** | T-158 | Power off and on pass; the reasons for Tunnels and Files are false |
| F12 Settings and palette (including "Lock here") | **FAIL** | T-152, T-157 | Almost all actions do what they say; "Lock here" locks but the UI never shows it and "Leave desk" leaves the other machine paired |
| F13 Updates (test latest.json, plan for both) | **PASS** |  | F13-updates · real release documented in `docs/dev/release.md` |
| F14 Tray | **PASS** | T-154 (related) | F14-tray · the tray says "0 tunnels" with tunnels open (T-154) |
| G2 Nothing fixed to Mac/PC (Windows+Windows, macOS+macOS) | **PASS** |  | G2-no-fixed-Mac-PC |
| G6 Every interactive element does something real | **FAIL** | T-150, T-153 | Button sweep: "Test crossing" always disabled and "Measure" disabled after reload |
| G7 / G8 Animations and reduced motion | **PASS** |  | G7-animations, G8-reduced-motion (sample of screens) |
| G9 Console without errors / without `{{`, `undefined`, `NaN` | **PASS** |  | No console with errors during the whole live run and the demo-mode sweep |
| G10 Everything editable is edited from the UI | **FAIL** | T-159 | Wake-on-LAN and the desk name cannot be edited from the UI |

Not evaluated here: P1–P15 and similarity with the mockups (part A), G1/G3–G5/G11 baseline quality (part C) and H1–H5 (hardware).

## Fix tasks

| Task | Failure | Flow |
|---|---|---|
| T-150 | Arrangement and crossing test without what the engine knows (real crossing, real screens) | F1, F2, G6 |
| T-151 | Setup step 4: detects what is assumed and does not hear the other machine | F1 |
| T-152 | "Leave desk" leaves the other machine paired | F12 |
| T-153 | Non-reactive `can()`: buttons disabled after reload | F6, G6 |
| T-154 | Empty tunnels in a freshly opened window | F5, F14 |
| T-155 | files_put of a credential answers "invalid manifest" | F5 |
| T-156 | Setup does not open by itself on Windows and "Close" is collapsed | F1 |
| T-157 | "Lock here" without a visible state | F12 |
| T-158 | Tunnels and Files modules with a false reason | F11, F1 |
| T-159 | Wake-on-LAN only through `tandem.toml`; desk name not editable | F10, G10 |

## Reproduce

1. `cargo build -p tandemd --features sim-backends -p tandem-cli` and, with the sources from `harness/` copied to `crates/tandemd/examples/acc_host.rs` and `apps/tandem-ui/src-tauri/examples/acc_bridge.rs`, `cargo build -p tandemd --features sim-backends --example acc_host` and `cargo build -p tandem-ui --example acc_bridge`; `pnpm -C apps/tandem-ui build` and `build:demo`.
2. Copy the four binaries (`tandemd`, `tandem`, `acc_host`, `acc_bridge`) to `/tmp/claude-1000/accB/bin` (the path used by `lib.mjs`; change it in `bin()`).
3. `node up.mjs <tag> '{"host":true}'` brings up the pair; the `f*.mjs` scripts take the `<tag>`; `all.sh` chains the official run.

## Detail of each check

### F1-setup

| Check | Result | Detail |
|---|---|---|
| first run on a non-macOS machine: setup opens by itself | **FAIL** | does not open by itself; you have to go to Settings → "Run setup again" |
| both UIs show the same 6-digit code | PASS | UI m2 shows 009207; agent m1 {"code":"009207","confirmed":false,"kind":"comparing","peer":"PC"} |
| a wrong code is rejected with a notice | PASS |  |
| paired: both agents say so, with different sides | PASS | m1=a m2=b |
| step 4: activity on the other machine is marked "Activity detected" | **FAIL** | you are using. ¦ Mac ¦ Keyboard and mouse ¦ PC ¦ Keyboard and mouse ¦ Back ¦ Continue |
| summary: the MCP command is copied | PASS | claude mcp add tandem -- tandem mcp |
| the setup "Close" button is visible and can be pressed | **FAIL** | box 24×2 px; what receives the click: brand svelte-1b2mlu3 |
| final summary: "Remote compute" reflects that tunnels work | **FAIL** | Your desk is ready ¦ This was really checked, not assumed. ¦ Mac and ¦ Simultaneo-1 in "My desk" ¦ 2 machines ¦ Link ¦ USB-C · 1.4 ms ¦ Keyboard and mouse ¦ crossing not tested ¦ Monitors ¦ Std-34 supported · owner Simultaneo-1 ¦ Remote compute ¦ remote comput |
| step 4: a machine without a keyboard or mouse connected does not appear with "Keyboard and mouse" detected | **FAIL** | hub_here m1/m2 = false,false; UI rows: ["Mac ¦ Keyboard and mouse","Simultaneo-1 ¦ Keyboard and mouse"] |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F1-setup/`

### F2-arrangement

| Check | Result | Detail |
|---|---|---|
| Mac «Right», alignment start: reaches both machines and tandem.toml | PASS |  |
| Mac «Right», alignment center: reaches both machines and tandem.toml | PASS |  |
| Mac «Right», alignment end: reaches both machines and tandem.toml | PASS |  |
| Mac «Top», alignment start: reaches both machines and tandem.toml | PASS |  |
| Mac «Top», alignment center: reaches both machines and tandem.toml | PASS |  |
| Mac «Top», alignment end: reaches both machines and tandem.toml | PASS |  |
| Mac «Bottom», alignment start: reaches both machines and tandem.toml | PASS |  |
| Mac «Bottom», alignment center: reaches both machines and tandem.toml | PASS |  |
| Mac «Bottom», alignment end: reaches both machines and tandem.toml | PASS |  |
| Mac «Left», alignment start: reaches both machines and tandem.toml | PASS |  |
| Mac «Left», alignment center: reaches both machines and tandem.toml | PASS |  |
| Mac «Left», alignment end: reaches both machines and tandem.toml | PASS |  |
| engine: with Mac «Right» of the PC the crossing through the new edge works (PC→Mac and Mac→PC) | PASS | PC→Mac true, Mac→PC true |
| engine: with Mac «Top» of the PC the crossing through the new edge works (PC→Mac and Mac→PC) | PASS | PC→Mac true, Mac→PC true |
| engine: the old edge («Right») no longer crosses | PASS |  |
| engine: with Mac «Bottom» of the PC the crossing through the new edge works (PC→Mac and Mac→PC) | PASS | PC→Mac true, Mac→PC true |
| engine: the old edge («Top») no longer crosses | PASS |  |
| engine: with Mac «Left» of the PC the crossing through the new edge works (PC→Mac and Mac→PC) | PASS | PC→Mac true, Mac→PC true |
| engine: the old edge («Bottom») no longer crosses | PASS |  |
| engine: the old edge («Left») no longer crosses | PASS |  |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F2-arrangement/`

### F3-scenes

| Check | Result | Detail |
|---|---|---|
| create scene: reaches tandem.toml on both machines | PASS | ["id = \"test-b\"\nname = \"Test B\"\nicon = \"desk\"\nmonitors = "] |
| create scene: returns to Monitors | PASS | #/monitors |
| create scene: appears as a card with its shortcut | PASS | Test B ¦ Std-34 → Mac ¦ ⌃⌥9 ¦ Apply |
| edit scene: the change reaches both machines | PASS | ["id = \"test-b\"\nname = \"Test B2\"\nicon = \"desk\"\nmonitors = "] |
| apply with success: the monitor really changed (double's bus) and progress shows it | PASS | bus={"std":["DisplayPort","Hdmi"],"fail":["DisplayPort","DisplayPort"]} ¦ Changes in progress ¦ Everything that touches another machine is a transaction: visible steps, a time limit and rollback if something fails. ¦ Applying the scene «Only Std» ¦ 0.0 s ¦ Sav |
| apply with success: the scene shows as "Active" in the UI of both machines | PASS | ["arrangement","cables","data","machines","modules","prefs","scenes","screens","shortcuts"] |
| apply with success: the other machine also sees it as "Active" | PASS |  |
| injected failure: the UI shows the rollback | PASS | Changes in progress ¦ Everything that touches another machine is a transaction: visible steps, a time limit and rollback if something fails. ¦ Applying the scene «Test B2» ¦ 0.0 s ¦ Save how everything is now ¦ done ¦ Std-34 → Mac (HDMI) ¦ undone ¦ Zfail-27 →  |
| injected failure: the desk returns to how it was (monitors on DP, previous scene) | PASS | {"std":["DisplayPort","DisplayPort"],"fail":["DisplayPort","DisplayPort"]} |
| global scene shortcut (Ctrl+Alt+7 on m1) applies the scene | PASS | before Hdmi,Hdmi → after DisplayPort,Hdmi |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F3-scenes/`

### F4-monitors

| Check | Result | Detail |
|---|---|---|
| factory monitor shows "Unsupported" with its reason | PASS |  |
| factory monitor: no DDC write on the bus | PASS | Set seen: 0; total ops 41 |
| the monitor's owner powers off: the other agent becomes the owner | PASS | owner before a, after b |
| fake standard monitor: changing the input from its card, with the monitor's verdict | PASS | bus: Hdmi → DisplayPort; card: Std-34 ¦ Tandem standard ¦ Simultaneo-1 · DP ¦ Simultaneo-1 ¦ DP ¦ Mac ¦ HDMI ¦ 10:46 ¦ Switch to DP confirmed ¦ 10:46 ¦ Signal detected ¦ 10:46 ¦ Input → DP, with the buttons |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F4-monitors/`

### F9-shared-config

| Check | Result | Detail |
|---|---|---|
| renaming a machine on A reaches B (agent and tandem.toml) | PASS | [["Escritorio-PC","Mac"],["Escritorio-PC","Mac"]] |
| B shows the notice "Changed from … · Undo" | PASS | Changed from Mac ¦ Machines and scenes |
| "Undo" restores the name on both machines | PASS | {"before":["PC","Mac"],"after":[["PC","Mac"],["PC","Mac"]]} |
| changing a machine's color on A reaches B | PASS | {"before":["#a99be8","#7aa7e6"],"after":[["#5fbfb0","#7aa7e6"],["#5fbfb0","#7aa7e6"]]} |
| changing the lock shortcut on A reaches B | PASS | ["lock = [\"Pause\"]","lock = [\"Pause\"]"] |
| changing a preference (Cmd↔Ctrl) on A reaches B | PASS | {"before":["[prefs] cmd_ctrl = true drag_files = true "],"after":["[prefs] cmd_ctrl = false drag_files = true ","[prefs] cmd_ctrl = false drag_files = true "]} |
| turning off "Shared clipboard" on A turns it off on both | PASS | ["off","off"] |
| a simultaneous change converges to the same value on both machines | PASS | [["Simultaneo-1","Mac"],["Simultaneo-1","Mac"]] |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F9-shared-config/`

### F6-network

| Check | Result | Detail |
|---|---|---|
| "Measure bandwidth" gives a value | PASS | Tandem ¦ Desk ¦ Tunnels ¦ Monitors ¦ Arrangement ¦ Network ¦ MACHINES ¦ Mac ¦ PC ¦ Settings ¦ Network ¦ Measure bandwidth ¦ Mac ¦ USB-C ¦ 1.5 ms ¦ Ethernet ¦ 1.3 ms ¦ Wi-Fi ¦ 1.3 ms ¦ PC ¦ Keyboard and mouse ¦ — ¦ no input ¦ Bandwidth ¦ 480 Mbps ¦ over USB-C ¦ |
| with two paths: primary USB-C and backup Wi-Fi | PASS | ms ¦ Switched from Ethernet to USB-C ¦ USB-C cable ¦ Primary ¦ Latency ¦ 1.2 ms ¦ Band ¦ — ¦ Ethernet ¦ Backup ¦ Latency ¦ 1.3 ms ¦ Band ¦ — ¦ Wi-Fi ¦ Backup ¦ Latency ¦ 1.4 ms ¦ Band ¦ — |
| cutting the primary path: input moves to the next one in < 1 s | PASS | 650 ms |
| the path-change event is visible in Network | PASS | d ¦ Measure bandwidth ¦ Simultaneo-1 ¦ USB-C · down ¦ Ethernet ¦ 1.7 ms ¦ Wi-Fi ¦ 1.6 ms ¦ Mac ¦ Keyboard and mouse ¦ — ¦ no input ¦ Bandwidth ¦ — ¦ not measured ¦ Latency · 60 s ¦ target 4 ms ¦ Cable unplugged · switched to Ethernet in 1.3 s ¦ USB-C cable ¦ D |
| after the cut "Measure bandwidth" measures the path carrying input (or explains why not) | PASS | jetivo 4 ms ¦ Spike when the cable was unplugged · switched to Ethernet in 1.3 s ¦ USB-C cable ¦ Down ¦ Latency ¦ — ¦ Band ¦ — ¦ Ethernet ¦ Primary ¦ Latency ¦ 1.3 ms ¦ Band ¦ 220 Mbps ¦ Wi-Fi ¦ Backup ¦ Latency ¦ 1.4 ms ¦ Band ¦ — ¦ Ethernet: 220 Mbps |
| when the cable returns, the primary path takes input back | PASS | path now: usb4 |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F6-network/`

### F5-tunnels-mcp

| Check | Result | Detail |
|---|---|---|
| P15: Claude's tools exist in `tandem mcp` | PASS | machines,resources,tunnels,tunnel_open,tunnel_close,exec,processes,output,kill,files_put,files_get,port_forward,port_stop,bring_back |
| tunnel_open with a git workspace | PASS | {"created":1795020952451,"id":"e7a204e50952d1af","opened_by":{"kind":"Mcp","session":"mcp"},"origin":"a","project":{"name":"proj-demo","root":"/tmp/claude-1000/accB/proj-demo"},"skipped":[".aws/credentials",".env",".npmrc","deploy.pem","id_rsa"],"state":"Open" |
| T-130: the other machine receives no credentials (.env, .pem, id_rsa, .npmrc, .aws) | PASS | files at destination: .git,src,src/lib.rs,src/new.rs ¦ filtered: [".aws/credentials",".env",".npmrc","deploy.pem","id_rsa"] ¦ notice: yes |
| T-130: the repo arrives without remotes or credential.helper | PASS | [core] repositoryformatversion = 0 filemode = true bare = false logallrefupdates = true [user] email = tandem@localhost name = Tandem |
| T-130: exec rejects environment variables that look like secrets | PASS | "the other machine did not accept it: the variable "GH_TOKEN" looks like a secret: the other machine does not receive credentials; explicit permission is needed in this call (allow_secret_env)" |
| T-130: with allow_secret_env only what was requested passes; GH_TOKEN and SSH_AUTH_SOCK from the origin never do | PASS | "GH= SSH= AWS=yy\n" |
| 3 processes running at once in the tunnel | PASS | running: 3 of [{"Exited":{"code":0}},"Running","Running","Running"] |
| a freshly opened UI shows the tunnels that already exist (without waiting for a change) | **FAIL** | ustes ¦ Tunnels ¦ No open tunnels ¦ How Claude opens them ¦ No open tunnels |
| the UI (origin) shows the project, the tunnel and its processes live | PASS | ustes ¦ Tunnels ¦ 1 open · 1 project ¦ How Claude opens them ¦ proj-demo ¦ /tmp/claude-1000/accB/proj-demo ¦ 1/1 ¦ ws-acceptance ¦ idle 0 s ¦ Mac ¦ PC ¦ proj-demo ¦ ws-acceptance ¦ idle 0 s ¦ Mac ¦ Claude session ¦ 0 B ¦ 0 B ¦ PC ¦ Windows ¦ Open ¦ 7 s ¦ Comma |
| the other machine's UI also shows the tunnel | PASS | ustes ¦ Tunnels ¦ 1 open · 1 project ¦ How Claude opens them ¦ proj-demo ¦ /tmp/claude-1000/accB/proj-demo ¦ 1/1 ¦ ws-acceptance ¦ idle 2 s ¦ Mac ¦ PC ¦ proj-demo ¦ ws-acceptance ¦ idle 2 s ¦ Mac ¦ Claude session ¦ 0 B ¦ 0 B ¦ PC ¦ Windows ¦ Open ¦ 12 s ¦ Comm |
| read each process's output, without mixing | PASS | ["p1-1,p1-2,p1-3,p1-4,p1-5,p1-6,","p2-1,p2-2,p2-3,p2-4,p2-5,p2-6,","p3-1,p3-2,p3-3,p3-4,p3-5,p3-6,"] |
| copy a file into the tunnel (files_put) | PASS | {"bytes":17,"dirs":0,"files":1,"sent":17,"skipped":[]} |
| T-130: files_put of a credential is skipped with a notice | **FAIL** | "the other machine: invalid manifest: a file copy carries exactly one entry" |
| fetch a file from the tunnel (files_get) | PASS | {"bytes":20,"dirs":0,"files":1,"sent":0,"skipped":[]} |
| open a port (port_forward) and talk through it | PASS | {"direction":"LocalToRemote","id":1,"local_port":42677,"remote_host":"127.0.0.1","remote_port":41677,"tunnel":"e7a204e50952d1af"} → eco:hello |
| cut the link and recover it without losing output (24 consecutive lines) | PASS | lines 24; during the cut: responded |
| bring_back creates the branch tandem/<name> in the project | PASS | {"branch":"tandem/ret","commits":[{"author":"t","id":"1f411dceb7af11e38afe1f308501deb4426c13f4","summary":"from-b"}],"committed_pending":false} ¦ branches: tandem/ret |
| bring_back: the branch carries the change made on the other machine | PASS |  |
| a UI opened (or reloaded) after opening a tunnel shows it without waiting for another change | **FAIL** | INAS ¦ Simultaneo-1 ¦ Mac ¦ Settings ¦ Tunnels ¦ No open tunnels ¦ How Claude opens them ¦ No open tunnels |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F5-tunnels-mcp/`

### F10-power

| Check | Result | Detail |
|---|---|---|
| power off the other machine: transaction with steps in the agent (PowerOff) and state off | PASS | steps: [[],["{\"step\":\"power_off\",\"machine\":\"a\"}"]] state {"on":{"a":false,"b":true},"scene":"trabajo"} |
| power on the other machine (simulated Wake-on-LAN over Ethernet): PowerOn step and state on | PASS | steps: [["{\"step\":\"power_on\",\"machine\":\"a\"}"],["{\"step\":\"power_off\",\"machine\":\"a\"}"]] state {"on":{"a":true,"b":true},"scene":null} |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F10-power/`

### F11-modules

| Check | Result | Detail |
|---|---|---|
| turn a module of the other machine on/off from its page (Clipboard) | PASS | other: active → off; (this: active) |
| turn "Keyboard and mouse" of the other machine off and on again | PASS | active → off → active |
| correct "unavailable" reasons: Tunnels does not say "not yet" if it already works | **FAIL** | Tunnels: unavailable · «Not yet in this version of Tandem» (tunnels worked in F5) |
| correct "unavailable" reasons: Files (file drag, T-121) | **FAIL** | Files: unavailable · «Not yet in this version of Tandem» |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F11-modules/`

### F12-settings-palette

| Check | Result | Detail |
|---|---|---|
| the palette lists all D15 commands | PASS | missing: |
| palette "Monitors" navigates to #/monitors | PASS | #/monitors |
| palette "Network" navigates to #/network | PASS | #/network |
| palette "Arrangement" navigates to #/arrangement | PASS | #/arrangement |
| palette "Settings" navigates to #/settings | PASS | #/settings |
| palette "Tunnels" navigates to #/tunnels | PASS | #/tunnels |
| palette "Desk" navigates to #/desk | PASS | #/desk |
| Ctrl+, opens Settings | PASS |  |
| palette "Open setup" opens it | PASS |  |
| palette "Change the other machine…" opens setup at "Your desk" | PASS | 2 Your desk |
| palette "Measure the link" asks the agent for the measurement and the UI gives the value or the reason | PASS | #/settings INAS ¦ Simultaneo-1 ¦ Mac ¦ Settings ¦ Settings ¦ Desk and machines ¦ Shortcuts ¦ Keyboard and mouse ¦ Updates ¦ Advanced ¦ Desk name ¦ My desk ¦ Simultaneo-1 ¦ Mac · this machine ¦ Change the other machine ¦ Pair again ¦ V |
| "Lock here" (on the machine that has the keyboard): the agent locks and the cursor does not cross; "Remove lock" releases it | PASS | crossed=true locked=true cruza_bloqueado=false cruza_tras_soltar=true |
| the UI indicates the cursor is locked (notice or visible mark) | **FAIL** | INAS ¦ Simultaneo-1 ¦ Mac ¦ Settings ¦ Desk ¦ Search or run… ¦ Ctrl+K ¦ Simultaneo-1 ¦ Windows ¦ CPU ¦ 12 % ¦ RAM ¦ 2.9/23.5 GB ¦ VRAM ¦ 1/15.9 GB ¦ Mac ¦ macOS ¦ CPU ¦ 12 % ¦ RAM ¦ 2.9/23.5 GB ¦ VRAM ¦ 1/15.9 GB ¦ Simultaneo-1 ¦ Mac ¦ USB-C ¦ — · — ¦ Input ¦  |
| "Lock here" on the machine that is not in control explains why it cannot be done | PASS | INAS ¦ Simultaneo-1 ¦ Mac ¦ Settings ¦ Desk ¦ Search or run… ¦ Ctrl+K ¦ Simultaneo-1 ¦ Windows ¦ CPU ¦ 11 % ¦ RAM ¦ 2.9/23.5 GB ¦ VRAM ¦ 1/15.9 GB ¦ Mac ¦ macOS ¦ CPU ¦ 11 % ¦ RAM ¦ 2.9/23.5 GB ¦ VRAM ¦ 1/15.9 GB ¦ Simultaneo-1 ¦ Mac ¦ USB-C ¦ — · — ¦ Input ¦  |
| Shortcuts: changing "Show or hide Tandem" is saved in ui.toml | PASS | [shortcuts] toggle_window = "Ctrl+Alt+Digit8" |
| Settings → "Leave desk" (with confirmation): both machines forget the keys | **FAIL** | peer m1=unpaired m2=paired (m2 did not unpair in 20 s); trust.toml bytes=21,114 |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F12-settings-palette/`

### F7-no-owner

| Check | Result | Detail |
|---|---|---|
| physical input on A and then on B: the one in use takes control | PASS | server after using m1=b, m2=a, m1=b (m1=b, m2=a) |
| when the owner changes with a key held, it does not stay stuck on the other machine | PASS | injected before: ["Key { usage: HidUsage { page: 7, id: 4 }, state: Pressed }"]; after: ["Key { usage: HidUsage { page: 7, id: 4 }, state: Released }","Key { usage: HidUsage { page: 7, id: 4 }, state: Pressed }"] |
| when the link drops with a key held, the other machine releases it | PASS | new events after the cut: ["Key { usage: HidUsage { page: 7, id: 5 }, state: Released }"] |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F7-no-owner/`

### F8-clipboard

| Check | Result | Detail |
|---|---|---|
| copy text A→B and paste it on the other | PASS | {"image":null,"text":"text-A→B","writes":1} |
| copy text B→A and paste it on the other | PASS | {"image":null,"text":"text-B→A","writes":1} |
| copy an image on A and paste it on B | PASS | {"image":[4,3],"text":null,"writes":2} |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F8-clipboard/`

### F13-trouble-states

| Check | Result | Detail |
|---|---|---|
| permissions revoked: the UI says so and offers how to grant them | PASS | INAS ¦ Simultaneo-1 ¦ Mac ¦ Settings ¦ Desk ¦ Search or run… ¦ Ctrl+K ¦ Permissions missing on this machine ¦ Review permissions |
| permissions granted: the Desk returns to normal | PASS |  |
| link down (the other is still on): the UI says "No link with …" and offers to retry | PASS | INAS ¦ Simultaneo-1 ¦ Mac ¦ Settings ¦ Desk ¦ Search or run… ¦ Ctrl+K ¦ No link with Simultaneo-1 ¦ Try again |
| link returns: the Desk recovers by itself | PASS |  |
| other machine lost: correct trouble state with its action | PASS | INAS ¦ Simultaneo-1 ¦ Mac ¦ Settings ¦ Desk ¦ Search or run… ¦ Ctrl+K ¦ Simultaneo-1 is off ¦ Power on Simultaneo-1 |
| the other returns: the Desk recovers by itself | PASS | INAS ¦ Simultaneo-1 ¦ Mac ¦ Settings ¦ Desk ¦ Search or run… ¦ Ctrl+K ¦ Simultaneo-1 ¦ Windows ¦ CPU ¦ 8 % ¦ RAM ¦ 2.5/23.5 GB ¦ VRAM ¦ |
| no agent: the UI explains that tandemd is not running | PASS | tes ¦ Desk ¦ Search or run… ¦ Ctrl+K ¦ tandemd is not running: nobody is listening on the local IPC |
| when the agent returns, the UI reconnects by itself | PASS | INAS ¦ Simultaneo-1 ¦ Mac ¦ Settings ¦ Desk ¦ Search or run… ¦ Ctrl+K ¦ Simultaneo-1 ¦ Windows ¦ CPU ¦ 9 % ¦ RAM ¦ 2.7/23.5 GB ¦ VRAM ¦ |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F13-trouble-states/`

### F13-updates

| Check | Result | Detail |
|---|---|---|
| shows the versions of both machines | PASS | Tandem 0.1.0 ¦ All machines on the desk are on the same version. ¦ Check now ¦ Update only ¦ Source ¦ GitHub Releases · unsigned |
| checking against a test latest.json finds the new version | PASS | Tandem 0.1.0 ¦ All machines on the desk are on the same version. ¦ Check again ¦ A new version is available: 0.2.0 ¦ It is installed on all machines, one at a time, without cutting the keyboard. ¦ Update all ¦ Update only ¦ Source ¦ GitHub Releases · unsigned |
| "Update all": first asks the other machine through tandemd (plan for both) | PASS | UI: Tandem 0.1.0 ¦ All machines on the desk are on the same version. ¦ Check again ¦ A new version is available: 0.2.0 ¦ It is installed on all machines, one at a time, without cutting the keyboard. ¦ Update all ¦ Actualizar ¦ agent B: [] ¦ B.update={"auto":tr |
| if the other does not come back with the new version, this one does not update and the UI explains it (rollback) | PASS | Tandem 0.1.0 ¦ All machines on the desk are on the same version. ¦ Check again ¦ A new version is available: 0.2.0 ¦ It is installed on all machines, one at a time, without cutting the keyboard. ¦ Update all ¦ Update only ¦ Source ¦ GitHub Releases · unsigned  |
| a nonexistent latest.json shows the error when checking | PASS | Tandem 0.1.0 ¦ All machines on the desk are on the same version. ¦ Check again ¦ A new version is available: 0.2.0 ¦ It is installed on all machines, one at a time, without cutting the keyboard. ¦ Update all ¦ Update only ¦ Source ¦ GitHub Releases · unsigned |
| "Update only" is saved in update.toml | PASS | auto = false |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F13-updates/`

### F14-tray

| Check | Result | Detail |
|---|---|---|
| the tray mini scene shows the desk state | PASS | My desk ¦ USB-C ¦ Simultaneo-1 ¦ 0 tunnels ¦ Mac ¦ macOS ¦ Scenes ¦ Work ¦ Only Simultaneo-1 ¦ Only Mac ¦ Test B2 ¦ Only Std ¦ Std to PC ¦ Open |
| applying a scene from the tray applies it in the agent | PASS | Only Std: work → only-std |
| "Open Tandem" and "Quit" in the tray reach the app core (ui_show_main / ui_quit) | PASS | ["ui_show_main","ui_quit"] |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F14-tray/`

### P6-crossing-test-demo

| Check | Result | Detail |
|---|---|---|
| demo: "crossed OK" state of the crossing test | PASS | … ¦ 34" · PC-Desktop ¦ the cursor arrived here ¦ Crossed OK ¦ Entered PC-Desktop at the correct height (y = 612 of 1440). Crossing latency: 0.4 ms. ¦ Try again ¦ Done |
| demo: "wrong edge" state of the crossing test | PASS | … ¦ 34" · PC-Desktop ¦ waiting for the cursor ¦ Crossed through another edge ¦ Crossed through the top, not the right: the arrangement does not match your desktop. Move it in the arrangement and try again. ¦ Edit |
| demo: "timed out" state of the crossing test | PASS | … ¦ 34" · PC-Desktop ¦ waiting for the cursor ¦ Did not cross in 20 s ¦ Maybe the 34" is on another side of your desktop. Move it in the arrangement and try again. ¦ Edit arrangement ¦ Try again |
| demo: "waiting" state of the crossing test | PASS | … ¦ 34" · PC-Desktop ¦ the cursor arrived here ¦ Crossed OK ¦ Entered PC-Desktop at the correct height (y = 612 of 1440). Crossing latency: 0.4 ms. ¦ Try again ¦ Done |

Screenshots: `docs/reports/acceptance/2026-11-18-b/P6-crossing-test-demo/`

### G8-reduced-motion

| Check | Result | Detail |
|---|---|---|
| with prefers-reduced-motion: reduce there are no animations or transitions | PASS |  |

Screenshots: `docs/reports/acceptance/2026-11-18-b/G8-reduced-motion/`

### G7-animations

| Check | Result | Detail |
|---|---|---|
| with normal motion there are animated entrances/transitions on the Desk | PASS | 39 elements animated |

Screenshots: `docs/reports/acceptance/2026-11-18-b/G7-animations/`

### F2-live-crossing-test

| Check | Result | Detail |
|---|---|---|
| Arrangement: "Test crossing" is available with the real agent | **FAIL** | the button is disabled (store.can('crossing') always false in LiveSource) |
| crossing test with a real CursorSample from the engine: it can be completed | **FAIL** | sor is here ¦ Simultaneo-1 ¦ waiting for the cursor ¦ The crossing cannot be tested ¦ The agent does not report the cursor crossing yet ¦ Edit arrangement |
| the agent does accept watch_cursor (what is missing is the UI) | PASS | {"kind":"done"} |

Screenshots: `docs/reports/acceptance/2026-11-18-b/F2-live-crossing-test/`

### G6-buttons-with-effect

| Check | Result | Detail |
|---|---|---|
| reloading the window on Network leaves "Measure bandwidth" enabled | **FAIL** | stays disabled forever (can() is not reactive) |
| every visible button on the live screens produces an observable effect | **FAIL** | 81 of 92 buttons have an effect; of the 11 without one, 10 are already-selected options (tabs, monitor input, screen, color) or the palette shortcut, disabled by design; the only button that does nothing real is "Test crossing" in Arrangement (always disabled, |

Screenshots: `docs/reports/acceptance/2026-11-18-b/G6-buttons-with-effect/`

### P10-notices

| Check | Result | Detail |
|---|---|---|
| a file copy from a tunnel shows a notice with progress in the UI | PASS | notices seen: ["Sending "big.bin" to Simultaneo-1 ¦ 0 % · 0 KB of 63 MB","Sending "big.bin" to Simultaneo-1 ¦ 3 % · 2 MB of 63 MB"] |

Screenshots: `docs/reports/acceptance/2026-11-18-b/P10-notices/`

### G2-no-fixed-Mac-PC

| Check | Result | Detail |
|---|---|---|
| Windows+Windows pair: no screen names the other system or uses the other system's shortcuts | PASS |  |
| macOS+macOS pair: no screen names the other system or uses the other system's shortcuts | PASS |  |

Screenshots: `docs/reports/acceptance/2026-11-18-b/G2-no-fixed-Mac-PC/`

### DEMO-sweep

| Check | Result | Detail |
|---|---|---|
| demo mode: no console errors, no {{ undefined NaN or Spanish text on any screen or variant | PASS |  |

Screenshots: `docs/reports/acceptance/2026-11-18-b/DEMO-sweep/`

## Other observations (no task)

- The "Changed from …" notices do not close by themselves (they ask for a decision) and cover controls in the top-right corner until dismissed; after "Undo", the notice that the other machine receives names the one that changed by its previous name.
- "Apply" on a scene whose monitors are not present says nothing (there is no notice that it did nothing).
- Scenes (F3): in 5 runs, 4 passed completely; in one the first application after adding the monitors failed with "it is not known which machine it shows (the real input or which input each machine uses is missing)" (the agent had not yet read the input or learned the cables), and in another the application went fine but the progress screen never showed (the transaction ended before opening it and went back to Monitors). They were not reduced to a single cause; if they reappear, they deserve their own task.
- The "link down" state resolved as "PC is off" when all three paths were cut for longer than `pc_lost_after_secs` (2 s in the test); with the default value it might look different: the range was not tested.
- "No agent" has no "Retry" button: it reconnects by itself every 2 s (`agent_retry` exists and the UI does not use it).
