# Keyboard and mouse engine (`tandem-input`)

Research of 2026-10-15. The code of lan-mouse (`f1b96bd`) and Deskflow (`c46781f`) was read, plus Microsoft's and Apple's documentation.
**Decision: write it from scratch.** lan-mouse and Deskflow serve only as a reference; **no code is copied from either**, because both are GPL.

## Why not use lan-mouse

- It is GPL-3.0-or-later (crates `input-capture`, `input-emulation` and `input-event` 0.4.0). Using it would force Tandem to be GPL too.
- It has gaps right on our two platforms:
  - On Windows movement is relative and with acceleration.
  - It does not handle UIPI, UAC or the secure desktop.
  - It has no media keys or clipboard.
  - On macOS it detects edges with the box that encloses all screens.
  - It has open bugs with modifiers, scroll and key repeat.
- Its event model is based on evdev (Linux) and needs a tokio `LocalSet`.

## Event model

- **Keys:** physical **USB HID** code (page 0x07, and 0x0C for media), equivalent to `KeyboardEvent.code`. The HID ↔ Mac keycode ↔ Windows scancode tables come from the `keycode` crate (MIT, Chromium's table).
- **Physical pass-through:** the destination system applies its own keyboard layout, so ES/LatAm, dead keys and AltGr/Option work on their own. Converting to characters stays only as an optional fallback.
- **Modifiers:** configurable remapping per direction. By default Mac→Win: Cmd→Ctrl, Ctrl→Win, Option→Alt, right Option→AltGr; the reverse for Win→Mac. The set of pressed keys is tracked and **all are released** on leaving, on disconnecting or on timeout.
- **Scroll:** two kinds, v120 for the wheel and pixels for the trackpad. Respect the sign of "natural scrolling".
- **Careful with the ISO Mac:** the `§`/`` ` `` keys are swapped with respect to HID. It has to be checked with the M5's keyboard.

## macOS

- **Capture:** `CGEventTapCreate` on `kCGHIDEventTap`, active tap (returning NULL swallows the event), on its own thread with its CFRunLoop. Re-enable it when `kCGEventTapDisabledByTimeout` arrives.
- **Permissions (TCC):** Accessibility for an active tap and for injecting; Input Monitoring if the tap is listen-only. The permission is tied to the binary's signature, so a stably signed `.app` is needed.
- **Freezing the cursor:** `CGDisplayHideCursor` + `SetsCursorInBackground` (private API) + `CGAssociateMouseAndMouseCursorPosition(false)`, as Deskflow does. It is better than lan-mouse, which repositions the cursor on every movement.
- **Secure Event Input** (password fields): detected with `IsSecureEventInputEnabled()`. A notice is shown in the UI and the screen is not crossed while it is active.
- **Injection:**
  - Keyboard: `CGEventCreateKeyboardEvent` + `CGEventPost(kCGHIDEventTap)`, also sending `kCGEventFlagsChanged` with the correct flags. Arrows carry `NumericPad|SecondaryFn`. Repeat is marked with `kCGKeyboardEventAutorepeat`.
  - Mouse: `*Dragged` events while a button is pressed; `kCGMouseEventClickState` for double and triple click.
  - Scroll: `CGEventCreateScrollWheelEvent2` in pixels.
  - Media: `NSEvent` of type `SystemDefined` with subtype 8.

## Windows

- **Capture:** `WH_KEYBOARD_LL`/`WH_MOUSE_LL` hooks to swallow input, plus Raw Input (`RIDEV_INPUTSINK`) to read movement without acceleration.
  - **The hook is silently removed if it takes longer than `LowLevelHooksTimeout`** (maximum 1 s): dedicated thread, minimal callback that only enqueues, and a watchdog that tests itself.
  - Ignore events with `LLMHF_INJECTED`/`LLKHF_INJECTED`, or mark our own with `dwExtraInfo`.
  - Filter the fake Ctrl that Windows sends along with AltGr.
- **Injection:**
  - Keyboard: `SendInput` with `KEYEVENTF_SCANCODE` (+ `EXTENDEDKEY` for E0 keys).
  - Mouse: **absolute** (`MOVE|ABSOLUTE|VIRTUALDESK`, 0–65535), accumulating the fractions. The process has to declare itself **Per-Monitor-V2 DPI aware**.
  - Wheel: `mouseData` accepts fractions of 120.
- **UIPI and secure desktop:** a SYSTEM service duplicates `winlogon.exe`'s token, sets `TokenUIAccess=1` and launches the helper with `CreateProcessAsUser` in `winsta0`. The helper follows desktop changes with `OpenInputDesktop`/`SetThreadDesktop`. Ctrl+Alt+Del is sent with `SendSAS` from the service. All of this requires a signed (Authenticode) binary installed in Program Files.

## Geometry

- The edge **of the screen where the cursor is** is tested, never the box that encloses all of them.
- The position is mapped as a fraction along the shared edge.
- Margin at the corners, a shortcut to lock the cursor on the current screen, and speed scaled by DPI.
- The geometry is re-read when screens change: `CGDisplayRegisterReconfigurationCallback` on Mac, `WM_DISPLAYCHANGE` on Windows.

## Transport

- **QUIC (quinn):** movement and scroll go in **datagrams** (coalesced, with a sequence number and a periodic absolute resync); keys, buttons, enter/leave and clipboard go in a **reliable, ordered stream**.
- Heartbeat of ~1 s. When the link changes, all the client's keys and buttons are released.
- Targets: under 5 ms of added latency over cable and p99 under 10 ms over Wi-Fi.

## Clipboard

- `arboard` 3.7 for reading and writing: text, HTML and image.
- Own change detection: NSPasteboard's `changeCount` on Mac, `AddClipboardFormatListener` on Windows.
- Avoid the echo by remembering the hash of what we wrote ourselves. Large contents are sent lazily: the change is announced first and transferred on paste.

## Estimate (one experienced developer)

About **15–18 weeks** in total. The largest parts are Windows's secure desktop service (2–3 weeks) and each system's capture (1.5–2 weeks each).

## References (read-only, do not copy)

- lan-mouse: `input-capture/src/{macos.rs,windows/event_thread.rs}`, `input-emulation/src/{macos.rs,windows.rs}`.
- Deskflow: `src/lib/platform/{OSXComputer.mm,MSWindowsWatchdog.cpp,MSWindowsProcess.cpp,MSWindowsDesks.cpp,MSWindowsHook.cpp,MSWindowsKeyState.cpp}`.
- Microsoft: LowLevelKeyboardProc, SendInput, the uiAccess policy.
