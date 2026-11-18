# S2 — Removing and returning screens on macOS

> Status (2026-10-15): implemented in `tandem-display` (T-012), compiled in `macos-latest` CI.
> **Not yet tested on the real Mac**: what follows is the design and what has to be
> checked. The Mac's results are added in the final section.

## What is needed (DESIGN §8 and §8.1)

- **Ghost**: remove from the macOS desktop the external monitor that the PC is showing.
- **Built-in screen**: Off (default), Mirror or Extended.
- Never leave the Mac without a screen.

## APIs

| What | API | Public | Notes |
|---|---|---|---|
| List | `CGGetOnlineDisplayList` | yes | `CGDirectDisplayID` ids. Includes mirrors; may **omit** disabled ones. |
| Data | `CGDisplayIsBuiltin/IsMain/IsActive/MirrorsDisplay/VendorNumber/ModelNumber/SerialNumber/Bounds` | yes | |
| Model name | `CoreDisplay_DisplayCreateInfoDictionary` → `DisplayProductName` | **no** | `{ locale: name }` dictionary with the EDID name (`LC34G55T`). |
| Remove/return | `CGSConfigureDisplayEnabled(config, id, bool)` | **no** (SkyLight) | The one BetterDisplay and others use. It goes inside `CGBeginDisplayConfiguration` / `CGCompleteDisplayConfiguration`. |
| Mirror | `CGConfigureDisplayMirrorOfDisplay(config, id, master)` | yes | `master = kCGNullDirectDisplay` undoes the mirror. |

Decisions:

- **Direct FFI, no new crates.** The two private functions are resolved with `dlsym`
  (first in the loaded images, then with `dlopen` of SkyLight / CoreDisplay). If Apple
  removes them, the binary still loads: `set_enabled` returns `PrivateApiUnavailable` and the
  built-in screen falls back to **Mirror** (the fallback §8.1 asks for). The name stays
  `None` and the monitor cannot be identified by model.
- **Configuration scope: `kCGConfigureForSession`.** `ForAppOnly` would revert when the
  process exits; it looks like a safety net, but a diagnostic CLI could not leave anything
  applied and it is not clear that SkyLight honors that scope for disabled screens.
  Rollback is provided by Tandem (`restore_all`, `RestoreGuard`). Review after testing on the Mac.
- **"Active" = `CGDisplayIsActive` or mirrored.** In a hardware mirror group only the
  primary counts as active; the one it reflects is not a ghost.

## Logic (portable, `tandem_display::quartz::MacTopology`)

- It identifies monitors by the EDID model (`MonitorConfig::edid_model`), case-insensitively;
  the built-in one never matches. Two equal ones = `Ambiguous` error.
- **It remembers what it disabled** with its last snapshot: if macOS stops listing the screen, it
  can still be found and returned by its id.
- Before disabling: it rejects if it is the last active screen (`LastDisplay`); it undoes the
  mirrors that depend on it (if the built-in one mirrored the 34" and the 34" goes away, the built-in one
  becomes extended instead of staying black) and takes the screen itself out of its mirror group.
- Built-in mirror: it mirrors the primary external or, if the primary is the built-in one,
  the first active external.
- Everything is idempotent: repeating a step does not configure anything again.
- Tests with `FakeQuartz` (including a property one: with any sequence of steps and
  failures, the Mac is never left without a screen, nothing mirrors a disabled one and `restore_all`
  returns everything).

## Questions only the Mac can settle

1. Does `CGSConfigureDisplayEnabled` exist and work on the M5's current macOS, for an
   external over HDMI **and** for the built-in one?
2. Is a disabled screen still in `CGGetOnlineDisplayList`? (The logic supports both.)
3. When re-enabling it, does it keep the same `CGDirectDisplayID`?
4. Does `CoreDisplay_DisplayCreateInfoDictionary` exist on Apple Silicon and give `DisplayProductName`?
   If not, plan B: `NSScreen.localizedName` (active screens only) or IORegistry.
5. A screen at rest (display sleep) shows up as not active: does it affect the
   "last screen" rule?
6. Does the configuration survive the Mac sleeping/waking and the cable being unplugged?

## Results on the Mac

_Pending: run `cargo test -p tandem-display -- --ignored` on `mac-runner` (hardware-verifier
agent) and note here the macOS version and the answer to each question._
